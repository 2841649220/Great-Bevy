use alloc::{borrow::Cow, sync::Arc};
use core::{
    ops::{DerefMut, Range},
};
use std::thread::{self, ThreadId};

use bevy_diagnostic::{Diagnostic, DiagnosticMeasurement, DiagnosticPath, DiagnosticsStore};
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_platform::time::Instant;
use std::sync::Mutex;

use crate::render_resource::{
    Buffer, BufferDescriptor, BufferSlice, CommandEncoder, ComputePass, MapMode,
};
use crate::render_resource::wgpu_compat::{QuerySet, RenderPass};
use crate::renderer::{RenderAdapterInfo, RenderDevice, RenderQueue, WgpuWrapper};
use wgpu_types::{BufferSize, BufferUsages, Features};

use super::RecordDiagnostics;

// buffer offset must be divisible by 256, so this constant must be divisible by 32 (=256/8)
const MAX_TIMESTAMP_QUERIES: u32 = 256;
const MAX_PIPELINE_STATISTICS: u32 = 128;

struct DiagnosticsRecorderInternal {
    features: Features,
    current_frame: Mutex<FrameData>,
    submitted_frames: Vec<FrameData>,
    finished_frames: Vec<FrameData>,
}

/// Records diagnostics into [`QuerySet`]'s keeping track of the mapping between
/// spans and indices to the corresponding entries in the [`QuerySet`].
///
/// M1-4b-2: the timestamp/statistics queries are gone (the diligent path has
/// no query support) - CPU timings and value-buffer diagnostics are still
/// recorded.
#[derive(Resource)]
pub struct DiagnosticsRecorder(WgpuWrapper<DiagnosticsRecorderInternal>);

impl DiagnosticsRecorder {
    /// Creates the new `DiagnosticsRecorder`.
    pub fn new(
        adapter_info: &RenderAdapterInfo,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) -> DiagnosticsRecorder {
        let features = device.features();

        #[cfg(feature = "tracing-tracy")]
        {
            // M1-4b-2: no GPU timestamps exist on the diligent path - the
            // tracy GPU context is a no-op (returns None).
            let _ = super::tracy_gpu::new_tracy_gpu_context(adapter_info, device, queue);
        }
        let _ = adapter_info; // Prevent unused variable warnings when tracing-tracy is not enabled
        let _ = queue; // M1-4b-2: the timestamp period is not needed (no GPU timestamps)

        DiagnosticsRecorder(WgpuWrapper::new(DiagnosticsRecorderInternal {
            features,
            current_frame: Mutex::new(FrameData::new(device, features)),
            submitted_frames: Vec::new(),
            finished_frames: Vec::new(),
        }))
    }

    fn current_frame_mut(&mut self) -> &mut FrameData {
        self.0.current_frame.get_mut().expect("lock poisoned")
    }

    fn current_frame_lock(&self) -> impl DerefMut<Target = FrameData> + '_ {
        self.0.current_frame.lock().expect("lock poisoned")
    }

    /// Begins recording diagnostics for a new frame.
    pub fn begin_frame(&mut self) {
        let internal = &mut self.0;
        // M1-4b-2: every submitted frame finishes synchronously (there
        // is no GPU readback to wait for).
        while !internal.submitted_frames.is_empty() {
            let removed = internal.submitted_frames.swap_remove(0);
            internal.finished_frames.push(removed);
        }

        self.current_frame_mut().begin();
    }

    /// Copies data from [`QuerySet`]'s to a [`Buffer`], after which it can be downloaded to CPU.
    ///
    /// Should be called before [`DiagnosticsRecorder::finish_frame`].
    pub fn resolve(&mut self, encoder: &mut CommandEncoder) {
        self.current_frame_mut().resolve(encoder);
    }

    /// Finishes recording diagnostics for the current frame.
    ///
    /// The specified `callback` will be invoked when diagnostics become available.
    ///
    /// Should be called after [`DiagnosticsRecorder::resolve`],
    /// and **after** all commands buffers have been queued.
    pub fn finish_frame(
        &mut self,
        device: &RenderDevice,
        callback: impl FnOnce(RenderDiagnostics) + Send + Sync + 'static,
    ) {
        let internal = &mut self.0;
        internal
            .current_frame
            .get_mut()
            .expect("lock poisoned")
            .finish(callback);

        // reuse one of the finished frames, if we can
        let new_frame = match internal.finished_frames.pop() {
            Some(frame) => frame,
            None => FrameData::new(device, internal.features),
        };

        let old_frame = core::mem::replace(
            internal.current_frame.get_mut().expect("lock poisoned"),
            new_frame,
        );
        internal.submitted_frames.push(old_frame);
    }
}

impl RecordDiagnostics for DiagnosticsRecorder {
    fn record_f32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        assert_eq!(
            buffer.size(),
            BufferSize::new(4).unwrap(),
            "DiagnosticsRecorder::record_f32 buffer slice must be 4 bytes long"
        );
        assert!(
            buffer.buffer().usage().contains(BufferUsages::COPY_SRC),
            "DiagnosticsRecorder::record_f32 buffer must have BufferUsages::COPY_SRC"
        );

        self.current_frame_lock()
            .record_value(command_encoder, buffer, name.into(), true);
    }

    fn record_u32<N>(&self, command_encoder: &mut CommandEncoder, buffer: &BufferSlice, name: N)
    where
        N: Into<Cow<'static, str>>,
    {
        assert_eq!(
            buffer.size(),
            BufferSize::new(4).unwrap(),
            "DiagnosticsRecorder::record_u32 buffer slice must be 4 bytes long"
        );
        assert!(
            buffer.buffer().usage().contains(BufferUsages::COPY_SRC),
            "DiagnosticsRecorder::record_u32 buffer must have BufferUsages::COPY_SRC"
        );

        self.current_frame_lock()
            .record_value(command_encoder, buffer, name.into(), false);
    }

    fn begin_time_span<E: WriteTimestamp>(&self, encoder: &mut E, span_name: Cow<'static, str>) {
        self.current_frame_lock()
            .begin_time_span(encoder, span_name);
    }

    fn end_time_span<E: WriteTimestamp>(&self, encoder: &mut E) {
        self.current_frame_lock().end_time_span(encoder);
    }

    fn begin_pass_span<P: Pass>(&self, pass: &mut P, span_name: Cow<'static, str>) {
        self.current_frame_lock().begin_pass(pass, span_name);
    }

    fn end_pass_span<P: Pass>(&self, pass: &mut P) {
        self.current_frame_lock().end_pass(pass);
    }
}

struct SpanRecord {
    thread_id: ThreadId,
    path_range: Range<usize>,
    begin_timestamp_index: Option<u32>,
    end_timestamp_index: Option<u32>,
    begin_instant: Option<Instant>,
    end_instant: Option<Instant>,
    pipeline_statistics_index: Option<u32>,
}

struct FrameData {
    device: RenderDevice,
    timestamps_query_set: Option<QuerySet>,
    num_timestamps: u32,
    supports_timestamps_inside_passes: bool,
    supports_timestamps_inside_encoders: bool,
    pipeline_statistics_query_set: Option<QuerySet>,
    num_pipeline_statistics: u32,
    path_components: Vec<Cow<'static, str>>,
    open_spans: Vec<SpanRecord>,
    closed_spans: Vec<SpanRecord>,
    value_buffers: Vec<(Buffer, Cow<'static, str>, bool)>,
}

impl FrameData {
    fn new(
        device: &RenderDevice,
        _features: Features,
) -> FrameData {
        // M1-4b-2: the diligent path has no timestamp/statistics query
        // support - the query sets and their buffers are never created and
        // the GPU-time diagnostics are skipped (CPU timings and value
        // buffers still work).
        FrameData {
            device: device.clone(),
            timestamps_query_set: None,
            num_timestamps: 0,
            supports_timestamps_inside_passes: false,
            supports_timestamps_inside_encoders: false,
            pipeline_statistics_query_set: None,
            num_pipeline_statistics: 0,
            path_components: Vec::new(),
            open_spans: Vec::new(),
            closed_spans: Vec::new(),
            value_buffers: Vec::new(),
        }
    }

    fn begin(&mut self) {
        self.num_timestamps = 0;
        self.num_pipeline_statistics = 0;
        self.path_components.clear();
        self.open_spans.clear();
        self.closed_spans.clear();
    }

    fn write_timestamp(
        &mut self,
        encoder: &mut impl WriteTimestamp,
        is_inside_pass: bool,
    ) -> Option<u32> {
        // `encoder.write_timestamp` is unsupported on WebGPU.
        if !self.supports_timestamps_inside_encoders {
            return None;
        }

        if is_inside_pass && !self.supports_timestamps_inside_passes {
            return None;
        }

        if self.num_timestamps >= MAX_TIMESTAMP_QUERIES {
            return None;
        }

        let set = self.timestamps_query_set.as_ref()?;
        let index = self.num_timestamps;
        encoder.write_timestamp(set, index);
        self.num_timestamps += 1;
        Some(index)
    }

    fn write_pipeline_statistics(
        &mut self,
        encoder: &mut impl WritePipelineStatistics,
    ) -> Option<u32> {
        if self.num_pipeline_statistics >= MAX_PIPELINE_STATISTICS {
            return None;
        }

        let set = self.pipeline_statistics_query_set.as_ref()?;
        let index = self.num_pipeline_statistics;
        encoder.begin_pipeline_statistics_query(set, index);
        self.num_pipeline_statistics += 1;
        Some(index)
    }

    fn open_span(
        &mut self,
        name: Cow<'static, str>,
    ) -> &mut SpanRecord {
        let thread_id = thread::current().id();

        let parent = self.open_spans.iter().rfind(|v| v.thread_id == thread_id);

        let path_range = match &parent {
            Some(parent) if parent.path_range.end == self.path_components.len() => {
                parent.path_range.start..parent.path_range.end + 1
            }
            Some(parent) => {
                self.path_components
                    .extend_from_within(parent.path_range.clone());
                self.path_components.len() - parent.path_range.len()..self.path_components.len() + 1
            }
            None => self.path_components.len()..self.path_components.len() + 1,
        };

        self.path_components.push(name);

        self.open_spans.push(SpanRecord {
            thread_id,
            path_range,
            begin_timestamp_index: None,
            end_timestamp_index: None,
            begin_instant: None,
            end_instant: None,
            pipeline_statistics_index: None,
        });

        self.open_spans.last_mut().unwrap()
    }

    fn close_span(&mut self) -> &mut SpanRecord {
        let thread_id = thread::current().id();

        let iter = self.open_spans.iter();
        let (index, _) = iter
            .enumerate()
            .rfind(|(_, v)| v.thread_id == thread_id)
            .unwrap();

        let span = self.open_spans.swap_remove(index);
        self.closed_spans.push(span);
        self.closed_spans.last_mut().unwrap()
    }

    fn record_value(
        &mut self,
        command_encoder: &mut CommandEncoder,
        buffer: &BufferSlice,
        name: Cow<'static, str>,
        is_f32: bool,
    ) {
        let dest_buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some(&format!("render_diagnostic_{name}")),
            size: 4,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        command_encoder.copy_buffer_to_buffer(
            buffer.buffer(),
            buffer.offset(),
            &dest_buffer,
            0,
            Some(buffer.size().into()),
        );

        command_encoder.map_buffer_on_submit(&dest_buffer, MapMode::Read, .., |_| {});

        self.value_buffers.push((dest_buffer, name, is_f32));
    }

    fn begin_time_span(&mut self, encoder: &mut impl WriteTimestamp, name: Cow<'static, str>) {
        let begin_instant = Instant::now();
        let begin_timestamp_index = self.write_timestamp(encoder, false);

        let span = self.open_span(name);
        span.begin_instant = Some(begin_instant);
        span.begin_timestamp_index = begin_timestamp_index;
    }

    fn end_time_span(&mut self, encoder: &mut impl WriteTimestamp) {
        let end_timestamp_index = self.write_timestamp(encoder, false);

        let span = self.close_span();
        span.end_timestamp_index = end_timestamp_index;
        span.end_instant = Some(Instant::now());
    }

    fn begin_pass<P: Pass>(&mut self, pass: &mut P, name: Cow<'static, str>) {
        let begin_instant = Instant::now();

        let begin_timestamp_index = self.write_timestamp(pass, true);
        let pipeline_statistics_index = self.write_pipeline_statistics(pass);

        let span = self.open_span(name);
        span.begin_instant = Some(begin_instant);
        span.begin_timestamp_index = begin_timestamp_index;
        span.pipeline_statistics_index = pipeline_statistics_index;
    }

    fn end_pass(&mut self, pass: &mut impl Pass) {
        let end_timestamp_index = self.write_timestamp(pass, true);

        let span = self.close_span();
        span.end_timestamp_index = end_timestamp_index;

        if span.pipeline_statistics_index.is_some() {
            pass.end_pipeline_statistics_query();
        }

        span.end_instant = Some(Instant::now());
    }

    fn resolve(&mut self, _encoder: &mut CommandEncoder) {
        // M1-4b-2: no query sets exist on the diligent path - nothing to
        // resolve; the value-buffer copies were already recorded by
        // `record_value` at call time.
    }

    fn diagnostic_path(&self, range: &Range<usize>, field: &str) -> DiagnosticPath {
        DiagnosticPath::from_components(
            core::iter::once("render")
                .chain(self.path_components[range.clone()].iter().map(|v| &**v))
                .chain(core::iter::once(field)),
        )
    }

    fn finish(&mut self, callback: impl FnOnce(RenderDiagnostics) + Send + Sync + 'static) {
        // M1-4b-2: the GPU readback machinery is gone - the diagnostics are
        // reported synchronously with the CPU timings and the value-buffer
        // values (the `record_value` copies + blocking maps were executed by
        // `CommandEncoder::finish`).
        let mut diagnostics = Vec::new();

        for span in &self.closed_spans {
            if let (Some(begin), Some(end)) = (span.begin_instant, span.end_instant) {
                diagnostics.push(RenderDiagnostic {
                    path: self.diagnostic_path(&span.path_range, "elapsed_cpu"),
                    suffix: "ms",
                    value: (end - begin).as_secs_f64() * 1000.0,
                });
            }
        }

        for (buffer, diagnostic_path, is_f32) in self.value_buffers.drain(..) {
            let buffer = buffer.get_mapped_range();
            diagnostics.push(RenderDiagnostic {
                path: DiagnosticPath::from_components(
                    core::iter::once("render").chain(core::iter::once(diagnostic_path.as_ref())),
                ),
                suffix: "",
                value: if is_f32 {
                    f32::from_le_bytes((*buffer).try_into().unwrap()) as f64
                } else {
                    u32::from_le_bytes((*buffer).try_into().unwrap()) as f64
                },
            });
        }

        callback(RenderDiagnostics(diagnostics));
    }
}

/// Resource which stores render diagnostics of the most recent frame.
#[derive(Debug, Default, Clone, Resource)]
pub struct RenderDiagnostics(Vec<RenderDiagnostic>);

/// A render diagnostic which has been recorded, but not yet stored in [`DiagnosticsStore`].
#[derive(Debug, Clone, Resource)]
pub struct RenderDiagnostic {
    pub path: DiagnosticPath,
    pub suffix: &'static str,
    pub value: f64,
}

/// Stores render diagnostics before they can be synced with the main app.
///
/// This mutex is locked twice per frame:
///  1. in `PreUpdate`, during [`sync_diagnostics`],
///  2. after rendering has finished and statistics have been downloaded from GPU.
#[derive(Debug, Default, Clone, Resource)]
pub struct RenderDiagnosticsMutex(pub(crate) Arc<Mutex<Option<RenderDiagnostics>>>);

/// Updates render diagnostics measurements.
pub fn sync_diagnostics(mutex: Res<RenderDiagnosticsMutex>, mut store: ResMut<DiagnosticsStore>) {
    let Some(diagnostics) = mutex.0.lock().ok().and_then(|mut v| v.take()) else {
        return;
    };

    let time = Instant::now();

    for diagnostic in &diagnostics.0 {
        if store.get(&diagnostic.path).is_none() {
            store.add(Diagnostic::new(diagnostic.path.clone()).with_suffix(diagnostic.suffix));
        }

        store
            .get_mut(&diagnostic.path)
            .unwrap()
            .add_measurement(DiagnosticMeasurement {
                time,
                value: diagnostic.value,
            });
    }
}

pub trait WriteTimestamp {
    fn write_timestamp(&mut self, query_set: &QuerySet, index: u32);
}

impl WriteTimestamp for CommandEncoder {
    fn write_timestamp(&mut self, query_set: &QuerySet, index: u32) {
        if cfg!(target_os = "macos") {
            // When using tracy (and thus this function), rendering was flickering on macOS Tahoe.
            // See: https://github.com/bevyengine/bevy/issues/22257
            // The issue seems to be triggered when `write_timestamp` is called very close to frame
            // presentation.
            return;
        }
        CommandEncoder::write_timestamp(self, query_set, index);
    }
}

impl WriteTimestamp for RenderPass<'_> {
    fn write_timestamp(&mut self, query_set: &QuerySet, index: u32) {
        RenderPass::write_timestamp(self, query_set, index);
    }
}

impl WriteTimestamp for ComputePass<'_> {
    fn write_timestamp(&mut self, query_set: &QuerySet, index: u32) {
        ComputePass::write_timestamp(self, query_set, index);
    }
}

pub trait WritePipelineStatistics {
    fn begin_pipeline_statistics_query(&mut self, query_set: &QuerySet, index: u32);

    fn end_pipeline_statistics_query(&mut self);
}

impl WritePipelineStatistics for RenderPass<'_> {
    fn begin_pipeline_statistics_query(&mut self, query_set: &QuerySet, index: u32) {
        RenderPass::begin_pipeline_statistics_query(self, query_set, index);
    }

    fn end_pipeline_statistics_query(&mut self) {
        RenderPass::end_pipeline_statistics_query(self);
    }
}

impl WritePipelineStatistics for ComputePass<'_> {
    fn begin_pipeline_statistics_query(&mut self, query_set: &QuerySet, index: u32) {
        ComputePass::begin_pipeline_statistics_query(self, query_set, index);
    }

    fn end_pipeline_statistics_query(&mut self) {
        ComputePass::end_pipeline_statistics_query(self);
    }
}

pub trait Pass: WritePipelineStatistics + WriteTimestamp {
    const KIND: PassKind;
}

impl Pass for RenderPass<'_> {
    const KIND: PassKind = PassKind::Render;
}

impl Pass for ComputePass<'_> {
    const KIND: PassKind = PassKind::Compute;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum PassKind {
    Render,
    Compute,
}

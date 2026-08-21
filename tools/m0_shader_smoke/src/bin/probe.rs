//! Minimal repro probe: compose + validate + HLSL a single wgsl file.
//! Used to isolate naga backend behavior (panic location) on small shaders.
use std::collections::HashMap;
use std::path::PathBuf;

use naga::valid::{ValidationFlags, Validator};
use naga_oil::compose::{Composer, NagaModuleDescriptor, ShaderDefValue, ShaderType};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = &args[0];
    let src = std::fs::read_to_string(file).unwrap();
    let caps: naga::valid::Capabilities = wgpu_naga_bridge::features_to_naga_capabilities(
        wgpu_types::Features::all(),
        wgpu_types::DownlevelFlags::all(),
    );
    let mut composer = Composer::default().with_capabilities(caps);
    let defs = HashMap::<String, ShaderDefValue>::new();
    let module = composer
        .make_naga_module(NagaModuleDescriptor {
            source: &src,
            file_path: file,
            shader_type: ShaderType::Wgsl,
            shader_defs: defs,
            additional_imports: &[],
        })
        .unwrap();
    let info = Validator::new(ValidationFlags::all(), composer.capabilities)
        .validate(&module)
        .unwrap();
    let mut source = String::new();
    let mut binding_map = std::collections::BTreeMap::new();
    for (_, var) in module.global_variables.iter() {
        if let Some(b) = &var.binding {
            let mut target = naga::back::hlsl::BindTarget {
                space: b.group as u8,
                register: b.binding,
                binding_array_size: None,
                dynamic_storage_buffer_offsets_index: None,
                restrict_indexing: false,
            };
            if let naga::TypeInner::BindingArray { size, .. } = module.types[var.ty].inner {
                if matches!(size, naga::ArraySize::Dynamic) {
                    target.binding_array_size = Some(2048);
                }
            }
            binding_map.insert(*b, target);
        }
    }
    let options = naga::back::hlsl::Options {
        shader_model: naga::back::hlsl::ShaderModel::V6_6,
        binding_map,
        immediates_target: Some(naga::back::hlsl::BindTarget {
            space: 0,
            register: 0,
            binding_array_size: None,
            dynamic_storage_buffer_offsets_index: None,
            restrict_indexing: false,
        }),
        ..Default::default()
    };
    let po = naga::back::hlsl::PipelineOptions {
        entry_point: Some((naga::ShaderStage::Compute, "main".to_string())),
    };
    let mut w = naga::back::hlsl::Writer::new(&mut source, &options, &po);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        w.write(&module, &info, None).map(|_| ())
    }));
    match r {
        Ok(Ok(())) => println!("{source}"),
        Ok(Err(e)) => println!("HLSL error: {e:?}"),
        Err(p) => println!(
            "PANIC: {}",
            p.downcast_ref::<&str>().unwrap_or(&"<non-str>")
        ),
    }
}

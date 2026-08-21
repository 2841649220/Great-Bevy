//! Bidirectional mapping between `wgpu_types::TextureFormat` and Diligent
//! `TEXTURE_FORMAT` values.
//!
//! # Scope
//!
//! The table covers every `TextureFormat::` variant used by `bevy_render` /
//! `bevy_image` in this fork (verified by grepping the crate tree; the usage
//! evidence list lives in the M1-1 task report), plus the remaining plain
//! 8/16/32-bit RGBA formats Diligent exposes. Every Diligent name is copied
//! from the generated bindings (`_TEXTURE_FORMAT` enum in `bindings.rs`) and
//! the locked headers (`.diligent_research/GraphicsTypes.h` and
//! `third_party/DiligentEngine/Graphics/GraphicsEngine/interface/GraphicsTypes.h`);
//! no enum values are invented.
//!
//! # Non-bijective cases (documented behavior)
//!
//! | wgpu format(s) | Diligent format | Note |
//! |---|---|---|
//! | `Depth24Plus`, `Depth24PlusStencil8`, `Stencil8` | `TEX_FORMAT_D24_UNORM_S8_UINT` | This locked Diligent version has no stencil-only and no "plain D24" format. On D3D12 (this machine) wgpu itself lands `Depth24Plus` on `D24_UNORM_S8_UINT`, so that is the landing format. Reverse direction is canonicalized to `Depth24PlusStencil8`. |
//! | `Depth32FloatStencil8` | `TEX_FORMAT_D32_FLOAT_S8X24_UINT` | bijective (exists in this version) |
//! | `Astc { .. }` (all block sizes/channels) | - | No ASTC formats exist in this locked Diligent version (ASTC was removed from the engine with the mobile backends). Maps to `Err`. |
//! | `EacR11*`, `EacRg11*` | - | No EAC formats exist in this version. Maps to `Err`. |
//! | `NV12`, `P010`, `R64Uint` | - | No counterparts. Maps to `Err`. |
//!
//! # sRGB dual views
//!
//! bevy_render keeps a linear and an sRGB view of the same allocation
//! (`bevy_render/src/view/window/mod.rs:79,406` `add_srgb_suffix()`,
//! `bevy_render/src/view/mod.rs:1269` `Bgra8Unorm => &[Bgra8UnormSrgb]`).
//! Two styles are expressible:
//!
//! 1. Create the texture directly in the sRGB format (`Rgba8UnormSrgb`
//!    maps bijectively to `TEX_FORMAT_RGBA8_UNORM_SRGB`, ...);
//! 2. Create the texture in the base linear format and create the sRGB view
//!    through a `TextureViewDesc.Format` override - the helper
//!    [`srgb_view_format`] returns the Diligent sRGB format for a base
//!    format, and [`crate::device::RenderDevice::create_texture_view`]
//!    accepts the overridden desc (M1b consumes this for the swapchain
//!    Bgra8 + `add_srgb_suffix()` pair).

use diligent_sys::bindings as sys;
use wgpu_types::TextureFormat;

use crate::error::{Error, Result};

/// `TEX_FORMAT_*` enum value as the `TEXTURE_FORMAT` type the C API takes.
const fn t(v: sys::_TEXTURE_FORMAT) -> sys::TEXTURE_FORMAT {
    v as sys::TEXTURE_FORMAT
}

/// wgpu format -> Diligent `TEX_FORMAT_*`, the single source of truth.
///
/// Only the canonical wgpu format appears per row; the documented
/// non-bijective landings are added by [`to_diligent`] on top of this table.
const WGPU_TO_DILIGENT: &[(TextureFormat, sys::TEXTURE_FORMAT)] = &[
    (TextureFormat::R8Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_UNORM)),
    (TextureFormat::R8Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_SNORM)),
    (TextureFormat::R8Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_UINT)),
    (TextureFormat::R8Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_SINT)),
    (TextureFormat::R16Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_UINT)),
    (TextureFormat::R16Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_SINT)),
    (TextureFormat::R16Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_UNORM)),
    (TextureFormat::R16Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_SNORM)),
    (TextureFormat::R16Float, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_FLOAT)),
    (TextureFormat::Rg8Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_UNORM)),
    (TextureFormat::Rg8Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_SNORM)),
    (TextureFormat::Rg8Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_UINT)),
    (TextureFormat::Rg8Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_SINT)),
    (TextureFormat::R32Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_UINT)),
    (TextureFormat::R32Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_SINT)),
    (TextureFormat::R32Float, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_FLOAT)),
    (TextureFormat::Rg16Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_UINT)),
    (TextureFormat::Rg16Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_SINT)),
    (TextureFormat::Rg16Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_UNORM)),
    (TextureFormat::Rg16Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_SNORM)),
    (TextureFormat::Rg16Float, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_FLOAT)),
    (TextureFormat::Rgba8Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM)),
    (
        TextureFormat::Rgba8UnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB),
    ),
    (TextureFormat::Rgba8Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_SNORM)),
    (TextureFormat::Rgba8Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UINT)),
    (TextureFormat::Rgba8Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_SINT)),
    (TextureFormat::Bgra8Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM)),
    (
        TextureFormat::Bgra8UnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM_SRGB),
    ),
    (
        TextureFormat::Rgb9e5Ufloat,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB9E5_SHAREDEXP),
    ),
    (TextureFormat::Rgb10a2Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB10A2_UINT)),
    (
        TextureFormat::Rgb10a2Unorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB10A2_UNORM),
    ),
    (
        TextureFormat::Rg11b10Ufloat,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R11G11B10_FLOAT),
    ),
    (TextureFormat::Rg32Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_UINT)),
    (TextureFormat::Rg32Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_SINT)),
    (TextureFormat::Rg32Float, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_FLOAT)),
    (TextureFormat::Rgba16Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_UINT)),
    (TextureFormat::Rgba16Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_SINT)),
    (TextureFormat::Rgba16Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_UNORM)),
    (TextureFormat::Rgba16Snorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_SNORM)),
    (
        TextureFormat::Rgba16Float,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_FLOAT),
    ),
    (TextureFormat::Rgba32Uint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_UINT)),
    (TextureFormat::Rgba32Sint, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_SINT)),
    (
        TextureFormat::Rgba32Float,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_FLOAT),
    ),
    (TextureFormat::Depth16Unorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D16_UNORM)),
    (TextureFormat::Depth32Float, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT)),
    (
        TextureFormat::Depth32FloatStencil8,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT_S8X24_UINT),
    ),
    (
        TextureFormat::Bc1RgbaUnorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC1_UNORM),
    ),
    (
        TextureFormat::Bc1RgbaUnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC1_UNORM_SRGB),
    ),
    (
        TextureFormat::Bc2RgbaUnorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC2_UNORM),
    ),
    (
        TextureFormat::Bc2RgbaUnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC2_UNORM_SRGB),
    ),
    (
        TextureFormat::Bc3RgbaUnorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC3_UNORM),
    ),
    (
        TextureFormat::Bc3RgbaUnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC3_UNORM_SRGB),
    ),
    (TextureFormat::Bc4RUnorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC4_UNORM)),
    (TextureFormat::Bc4RSnorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC4_SNORM)),
    (TextureFormat::Bc5RgUnorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC5_UNORM)),
    (TextureFormat::Bc5RgSnorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC5_SNORM)),
    (
        TextureFormat::Bc6hRgbUfloat,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC6H_UF16),
    ),
    (
        TextureFormat::Bc6hRgbFloat,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC6H_SF16),
    ),
    (TextureFormat::Bc7RgbaUnorm, t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC7_UNORM)),
    (
        TextureFormat::Bc7RgbaUnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC7_UNORM_SRGB),
    ),
    (
        TextureFormat::Etc2Rgb8Unorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8_UNORM),
    ),
    (
        TextureFormat::Etc2Rgb8UnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8_UNORM_SRGB),
    ),
    (
        TextureFormat::Etc2Rgb8A1Unorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8A1_UNORM),
    ),
    (
        TextureFormat::Etc2Rgb8A1UnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8A1_UNORM_SRGB),
    ),
    (
        TextureFormat::Etc2Rgba8Unorm,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGBA8_UNORM),
    ),
    (
        TextureFormat::Etc2Rgba8UnormSrgb,
        t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGBA8_UNORM_SRGB),
    ),
];

/// wgpu formats that land on a Diligent format shared with another wgpu
/// format (reverse mapping canonicalizes to [`TextureFormat::Depth24PlusStencil8`]).
const NON_BIJECTIVE_WGPU: &[TextureFormat] = &[
    TextureFormat::Depth24Plus,
    TextureFormat::Stencil8,
];

/// Maps a wgpu format to the Diligent `TEX_FORMAT_*` value.
///
/// Returns [`Error::UnsupportedFormat`] for formats that have no counterpart
/// in this locked Diligent version (ASTC/EAC/NV12/P010/R64Uint - see module
/// docs).
pub fn to_diligent(f: TextureFormat) -> Result<sys::TEXTURE_FORMAT> {
    if let Some((_, d)) = WGPU_TO_DILIGENT.iter().find(|(w, _)| *w == f) {
        return Ok(*d);
    }
    // Documented non-bijective landing: Depth24Plus / Depth24PlusStencil8 /
    // Stencil8 share TEX_FORMAT_D24_UNORM_S8_UINT (see module docs).
    if NON_BIJECTIVE_WGPU.contains(&f) || f == TextureFormat::Depth24PlusStencil8 {
        return Ok(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D24_UNORM_S8_UINT));
    }
    Err(Error::UnsupportedFormat(
        "wgpu format has no Diligent counterpart",
    ))
}

/// Maps a Diligent `TEX_FORMAT_*` value back to a wgpu format.
///
/// The reverse direction canonicalizes the shared `D24_UNORM_S8_UINT`
/// landing to [`TextureFormat::Depth24PlusStencil8`]. Returns
/// [`Error::UnsupportedFormat`] for formats without a wgpu counterpart
/// (`UNKNOWN`, `*_TYPELESS`, ...).
pub fn from_diligent(f: sys::TEXTURE_FORMAT) -> Result<TextureFormat> {
    if let Some((w, _)) = WGPU_TO_DILIGENT.iter().find(|(_, d)| *d == f) {
        return Ok(*w);
    }
    match f {
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D24_UNORM_S8_UINT) => {
            Ok(TextureFormat::Depth24PlusStencil8)
        }
        _ => Err(Error::UnsupportedFormat(
            "Diligent format has no wgpu counterpart",
        )),
    }
}

/// The Diligent sRGB `TEX_FORMAT_*` for a base (linear) wgpu format, or
/// `None` when no sRGB twin exists.
///
/// This is the `TextureViewDesc.Format` override helper for the sRGB
/// dual-view semantics: create the texture in the base format and use the
/// returned value as the view format (see the module docs).
pub fn srgb_view_format(base: TextureFormat) -> Option<sys::TEXTURE_FORMAT> {
    match base {
        TextureFormat::Rgba8Unorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB)),
        TextureFormat::Bgra8Unorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM_SRGB)),
        TextureFormat::Bc1RgbaUnorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC1_UNORM_SRGB)),
        TextureFormat::Bc2RgbaUnorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC2_UNORM_SRGB)),
        TextureFormat::Bc3RgbaUnorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC3_UNORM_SRGB)),
        TextureFormat::Bc7RgbaUnorm => Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC7_UNORM_SRGB)),
        TextureFormat::Etc2Rgb8Unorm => {
            Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8_UNORM_SRGB))
        }
        TextureFormat::Etc2Rgb8A1Unorm => {
            Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGB8A1_UNORM_SRGB))
        }
        TextureFormat::Etc2Rgba8Unorm => {
            Some(t(sys::_TEXTURE_FORMAT::TEX_FORMAT_ETC2_RGBA8_UNORM_SRGB))
        }
        _ => None,
    }
}

/// Bytes per pixel for plain (non-block-compressed) Diligent formats.
///
/// Returns `None` for block-compressed formats (their size is per 4x4
/// block, not per pixel) and for formats outside the mapping table. Used by
/// [`crate::device::RenderDevice::create_texture`] to validate initial-data
/// sizes and to derive the upload row stride.
pub fn bytes_per_pixel(f: sys::TEXTURE_FORMAT) -> Option<u32> {
    let bpp = match f {
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_A8_UNORM) =>
        {
            1
        }
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R16_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D16_UNORM) =>
        {
            2
        }
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG16_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM_SRGB)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB10A2_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB10A2_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_R11G11B10_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB9E5_SHAREDEXP)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D24_UNORM_S8_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT) =>
        {
            4
        }
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_UNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_SNORM)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_SINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT_S8X24_UINT) =>
        {
            8
        }
        f if f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_FLOAT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_UINT)
            || f == t(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_SINT) =>
        {
            16
        }
        _ => return None,
    };
    Some(bpp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(f: sys::_TEXTURE_FORMAT) -> sys::TEXTURE_FORMAT {
        f as sys::TEXTURE_FORMAT
    }

    fn astc_4x4_srgb() -> TextureFormat {
        use wgpu_types::{AstcBlock, AstcChannel};
        TextureFormat::Astc {
            block: AstcBlock::B4x4,
            channel: AstcChannel::UnormSrgb,
        }
    }

    /// Every bijective table entry round-trips through both directions.
    #[test]
    fn round_trip_is_identity_for_bijective_pairs() {
        // The previous skip never fired: NON_BIJECTIVE_WGPU formats are
        // resolved outside the table (see `to_diligent`), never as rows.
        // Assert that invariant instead, so a future table edit fails
        // loudly rather than being silently skipped here.
        for w in NON_BIJECTIVE_WGPU {
            assert!(
                !WGPU_TO_DILIGENT.iter().any(|(row, _)| row == w),
                "{w:?} must not be a table row (it is resolved in `to_diligent`)"
            );
        }
        for (w, _) in WGPU_TO_DILIGENT {
            let diligent = to_diligent(*w).expect("to_diligent");
            assert_eq!(
                from_diligent(diligent).expect("from_diligent"),
                *w,
                "round trip failed for {w:?}"
            );
        }
    }

    /// The full table resolves in both directions (coverage test).
    #[test]
    fn coverage_table_resolves_both_ways() {
        for (w, d) in WGPU_TO_DILIGENT {
            assert_eq!(to_diligent(*w).expect("to_diligent"), *d);
            assert!(from_diligent(*d).is_ok(), "from_diligent failed for {d}");
        }
        assert!(
            WGPU_TO_DILIGENT.len() >= 60,
            "mapping table unexpectedly shrank: {} entries",
            WGPU_TO_DILIGENT.len()
        );
    }

    /// Depth24Plus / Depth24PlusStencil8 / Stencil8 share one landing format;
    /// the reverse direction canonicalizes to Depth24PlusStencil8.
    #[test]
    fn depth24plus_lands_on_d24_unorm_s8_uint() {
        let d24 = tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_D24_UNORM_S8_UINT);
        for w in [
            TextureFormat::Depth24Plus,
            TextureFormat::Depth24PlusStencil8,
            TextureFormat::Stencil8,
        ] {
            assert_eq!(to_diligent(w).expect("to_diligent"), d24);
        }
        assert_eq!(
            from_diligent(d24).expect("from_diligent"),
            TextureFormat::Depth24PlusStencil8
        );
    }

    /// Formats without a Diligent counterpart are rejected, not silently
    /// remapped.
    #[test]
    fn unsupported_wgpu_formats_are_rejected() {
        use wgpu_types::{AstcBlock, AstcChannel};
        for block in [
            AstcBlock::B4x4,
            AstcBlock::B5x4,
            AstcBlock::B5x5,
            AstcBlock::B6x5,
            AstcBlock::B6x6,
            AstcBlock::B8x5,
            AstcBlock::B8x6,
            AstcBlock::B8x8,
            AstcBlock::B10x5,
            AstcBlock::B10x6,
            AstcBlock::B10x8,
            AstcBlock::B10x10,
            AstcBlock::B12x10,
            AstcBlock::B12x12,
        ] {
            assert!(to_diligent(TextureFormat::Astc { block, channel: AstcChannel::Unorm }).is_err());
            assert!(to_diligent(TextureFormat::Astc { block, channel: AstcChannel::UnormSrgb }).is_err());
        }
        assert!(to_diligent(astc_4x4_srgb()).is_err());
        assert!(to_diligent(TextureFormat::EacR11Unorm).is_err());
        assert!(to_diligent(TextureFormat::EacR11Snorm).is_err());
        assert!(to_diligent(TextureFormat::EacRg11Unorm).is_err());
        assert!(to_diligent(TextureFormat::EacRg11Snorm).is_err());
        assert!(to_diligent(TextureFormat::NV12).is_err());
        assert!(to_diligent(TextureFormat::P010).is_err());
        assert!(to_diligent(TextureFormat::R64Uint).is_err());
    }

    /// Diligent formats with no wgpu counterpart (UNKNOWN, TYPELESS) are
    /// rejected in the reverse direction.
    #[test]
    fn unknown_diligent_formats_are_rejected() {
        assert!(from_diligent(tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN)).is_err());
        assert!(from_diligent(tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_TYPELESS)).is_err());
        assert!(from_diligent(tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_NUM_FORMATS)).is_err());
    }

    /// srgb_view_format returns the sRGB twin only for formats that have one.
    #[test]
    fn srgb_view_format_returns_srgb_twins() {
        let cases = [
            (
                TextureFormat::Rgba8Unorm,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB,
            ),
            (
                TextureFormat::Bgra8Unorm,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM_SRGB,
            ),
            (
                TextureFormat::Bc1RgbaUnorm,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_BC1_UNORM_SRGB,
            ),
            (
                TextureFormat::Bc3RgbaUnorm,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_BC3_UNORM_SRGB,
            ),
            (
                TextureFormat::Bc7RgbaUnorm,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_BC7_UNORM_SRGB,
            ),
        ];
        for (base, expected) in cases {
            assert_eq!(
                srgb_view_format(base).expect("srgb twin"),
                tex(expected),
                "srgb twin mismatch for {base:?}"
            );
        }
        assert_eq!(srgb_view_format(TextureFormat::R8Unorm), None);
        assert_eq!(srgb_view_format(TextureFormat::Depth32Float), None);
    }

    /// bytes_per_pixel spot checks for the formats the wrapper uploads.
    #[test]
    fn bytes_per_pixel_spot_checks() {
        let cases = [
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_R8_UNORM, 1),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RG8_UNORM, 2),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_BGRA8_UNORM, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA16_FLOAT, 8),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_R32_FLOAT, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RG32_FLOAT, 8),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA32_FLOAT, 16),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_R11G11B10_FLOAT, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_RGB10A2_UNORM, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_D16_UNORM, 2),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_D24_UNORM_S8_UINT, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT, 4),
            (sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT_S8X24_UINT, 8),
        ];
        for (f, expected) in cases {
            assert_eq!(bytes_per_pixel(tex(f)), Some(expected), "bpp mismatch for {f:?}");
        }
        assert_eq!(bytes_per_pixel(tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_BC7_UNORM)), None);
        assert_eq!(bytes_per_pixel(tex(sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN)), None);
    }
}

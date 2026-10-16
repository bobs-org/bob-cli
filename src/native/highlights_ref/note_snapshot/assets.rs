//! Decode and stage note snapshot images for the print profile.

use std::{io::Cursor, path::PathBuf};

use image::{imageops::FilterType, DynamicImage, ImageFormat};

use super::{
    plan::SnapshotPlan,
    view::{
        AssetMediaKind, SnapshotWarning, SnapshotWarningKind, SourceLocation,
    },
};

const MAX_EDGE: u32 = 1600;

#[derive(Clone, Debug)]
pub(crate) struct PreparedAsset {
    pub relative_path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AssetPreparation {
    pub assets: Vec<PreparedAsset>,
    pub downscaled: Vec<String>,
    pub missing: Vec<String>,
}

struct ProcessedImage {
    bytes: Vec<u8>,
    extension: &'static str,
    downscaled: bool,
}

/// Decode every supported embed once, updating the view for placeholders and
/// converted extensions. `prepare` is also used by dry runs; it writes no files.
pub(crate) fn prepare(
    snapshot: &mut SnapshotPlan,
) -> Result<AssetPreparation, String> {
    let mut result = AssetPreparation::default();
    for index in 0..snapshot.projection.view.assets.len() {
        let asset = snapshot.projection.view.assets[index].clone();
        let display = asset
            .vault_relative_path
            .to_string_lossy()
            .replace('\\', "/");

        if asset.media_kind == AssetMediaKind::Svg
            || asset.media_kind == AssetMediaKind::Other
        {
            mark_missing(snapshot, index, &display, &mut result);
            continue;
        }
        let source_bytes = std::fs::read(&asset.canonical_source_path).ok();
        let processed = source_bytes
            .as_deref()
            .and_then(|bytes| process_image(asset.media_kind, bytes));
        let Some(processed) = processed else {
            mark_missing(snapshot, index, &display, &mut result);
            continue;
        };
        if processed.downscaled {
            result.downscaled.push(display.clone());
        }
        let asset = &mut snapshot.projection.view.assets[index];
        if processed.extension
            != asset
                .relative_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        {
            asset.relative_path.set_extension(processed.extension);
        }
        asset.media_kind = if processed.extension == "jpg" {
            AssetMediaKind::Jpeg
        } else {
            AssetMediaKind::Png
        };
        result.assets.push(PreparedAsset {
            relative_path: asset.relative_path.clone(),
            bytes: processed.bytes,
        });
    }
    snapshot.projected_markdown =
        super::serialize_projected_markdown(&snapshot.projection.view)
            .map_err(|error| {
                format!(
                    "serialize note snapshot after image preparation: {error}"
                )
            })?;
    Ok(result)
}

fn process_image(
    kind: AssetMediaKind,
    source: &[u8],
) -> Option<ProcessedImage> {
    if matches!(
        kind,
        AssetMediaKind::Svg | AssetMediaKind::Other | AssetMediaKind::Missing
    ) {
        return None;
    }
    let image = image::load_from_memory(source).ok()?;
    let (width, height) = (image.width(), image.height());
    let downscaled = width.max(height) > MAX_EDGE;
    let image = if downscaled {
        let scale = MAX_EDGE as f64 / width.max(height) as f64;
        let new_width = ((width as f64 * scale).round() as u32).max(1);
        let new_height = ((height as f64 * scale).round() as u32).max(1);
        image.resize_exact(new_width, new_height, FilterType::Lanczos3)
    } else {
        image
    };
    let (bytes, extension) = match kind {
        AssetMediaKind::Png if !downscaled => (source.to_vec(), "png"),
        AssetMediaKind::Jpeg if !downscaled => (source.to_vec(), "jpg"),
        AssetMediaKind::Jpeg => encode(image, ImageFormat::Jpeg, "jpg")?,
        AssetMediaKind::Png | AssetMediaKind::Gif | AssetMediaKind::Webp => {
            encode(image, ImageFormat::Png, "png")?
        }
        AssetMediaKind::Svg
        | AssetMediaKind::Other
        | AssetMediaKind::Missing => return None,
    };
    Some(ProcessedImage {
        bytes,
        extension,
        downscaled,
    })
}

fn encode(
    image: DynamicImage,
    format: ImageFormat,
    extension: &'static str,
) -> Option<(Vec<u8>, &'static str)> {
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, format).ok()?;
    Some((cursor.into_inner(), extension))
}

fn mark_missing(
    snapshot: &mut SnapshotPlan,
    index: usize,
    display: &str,
    result: &mut AssetPreparation,
) {
    snapshot.projection.view.assets[index].media_kind = AssetMediaKind::Missing;
    snapshot.projection.view.assets[index].missing_label =
        Some(display.to_string());
    snapshot.summary.missing_images += 1;
    result.missing.push(display.to_string());
    snapshot.warnings.push(SnapshotWarning {
        kind: SnapshotWarningKind::MissingImage,
        origin: Some(SourceLocation {
            source: snapshot.note.identity.clone(),
            line: None,
        }),
        message: format!("image not printable · {display}"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(
            width,
            height,
            Rgb([40, 80, 120]),
        ));
        let mut cursor = Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, format)
            .expect("encode synthetic image");
        cursor.into_inner()
    }

    #[test]
    fn png_and_jpeg_are_preserved_and_oversized_images_are_downscaled() {
        for (kind, format, extension) in [
            (AssetMediaKind::Png, ImageFormat::Png, "png"),
            (AssetMediaKind::Jpeg, ImageFormat::Jpeg, "jpg"),
        ] {
            let small = encoded(24, 12, format);
            let prepared = process_image(kind, &small).expect("small image");
            assert_eq!(prepared.bytes, small);
            assert_eq!(prepared.extension, extension);
            assert!(!prepared.downscaled);

            let large = encoded(2000, 1000, format);
            let prepared = process_image(kind, &large).expect("large image");
            assert!(prepared.downscaled);
            let decoded = image::load_from_memory(&prepared.bytes)
                .expect("decode output");
            assert_eq!((decoded.width(), decoded.height()), (1600, 800));
        }
    }

    #[test]
    fn gif_and_webp_convert_to_png_and_unsupported_or_broken_images_fail() {
        for (kind, format) in [
            (AssetMediaKind::Gif, ImageFormat::Gif),
            (AssetMediaKind::Webp, ImageFormat::WebP),
        ] {
            let input = encoded(20, 10, format);
            let prepared = process_image(kind, &input).expect("convert image");
            assert_eq!(prepared.extension, "png");
            assert_eq!(
                image::guess_format(&prepared.bytes).expect("output format"),
                ImageFormat::Png
            );
        }
        assert!(process_image(AssetMediaKind::Svg, b"<svg/>").is_none());
        assert!(process_image(AssetMediaKind::Png, b"not an image").is_none());
    }
}

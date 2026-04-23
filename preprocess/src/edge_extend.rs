use crate::{
    dataset::PreprocessContext,
    gdal::{Dataset, raster::GdalType},
    result::PreprocessResult,
};
use bevy_terrain::math::TileCoordinate;

#[derive(Clone, Copy)]
struct TexelRect {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
}

fn valid_texel_rect(
    tile_coordinate: TileCoordinate,
    context: &PreprocessContext,
) -> Option<TexelRect> {
    let valid_uv_rect = context.valid_uv_rect?;
    let center_size = context.attachment.center_size() as i64;
    let border_size = context.attachment.border_size as i64;
    let tile_count = 2.0_f64.powi(tile_coordinate.lod as i32);
    let domain_size = tile_count * center_size as f64;

    let valid_min_x = valid_uv_rect.min[0].min(valid_uv_rect.max[0]) as f64;
    let valid_min_y = valid_uv_rect.min[1].min(valid_uv_rect.max[1]) as f64;
    let valid_max_x = valid_uv_rect.min[0].max(valid_uv_rect.max[0]) as f64;
    let valid_max_y = valid_uv_rect.min[1].max(valid_uv_rect.max[1]) as f64;

    let valid_start_x = (valid_min_x * domain_size).floor() as i64;
    let valid_start_y = (valid_min_y * domain_size).floor() as i64;
    let valid_end_x = (valid_max_x * domain_size).ceil() as i64;
    let valid_end_y = (valid_max_y * domain_size).ceil() as i64;

    let tile_start_x = tile_coordinate.xy.x as i64 * center_size;
    let tile_start_y = tile_coordinate.xy.y as i64 * center_size;

    let local_start_x = (valid_start_x - tile_start_x).clamp(0, center_size);
    let local_start_y = (valid_start_y - tile_start_y).clamp(0, center_size);
    let local_end_x = (valid_end_x - tile_start_x).clamp(0, center_size);
    let local_end_y = (valid_end_y - tile_start_y).clamp(0, center_size);

    if local_start_x >= local_end_x || local_start_y >= local_end_y {
        return None;
    }

    Some(TexelRect {
        min_x: (border_size + local_start_x) as usize,
        min_y: (border_size + local_start_y) as usize,
        max_x: (border_size + local_end_x - 1) as usize,
        max_y: (border_size + local_end_y - 1) as usize,
    })
}

pub(crate) fn edge_extend_tile<T: Copy + GdalType>(
    tile_coordinate: TileCoordinate,
    tile_dataset: &Dataset,
    context: &PreprocessContext,
) -> PreprocessResult<()> {
    let Some(valid_rect) = valid_texel_rect(tile_coordinate, context) else {
        return Ok(());
    };

    let texture_size = context.attachment.texture_size as usize;

    // Hidden clipped pixels still affect linear filtering and generated mips, so repeat the
    // nearest valid texel outward before stitching and mip generation see the tile.
    for band in tile_dataset.rasterbands() {
        let mut band = band?;
        let original = band.read_band_as::<T>()?;
        let mut extended = Vec::with_capacity(texture_size * texture_size);

        for y in 0..texture_size {
            let source_y = y.clamp(valid_rect.min_y, valid_rect.max_y);

            for x in 0..texture_size {
                let source_x = x.clamp(valid_rect.min_x, valid_rect.max_x);
                extended.push(original[(source_y, source_x)]);
            }
        }

        let mut buffer = crate::gdal::raster::Buffer::new((texture_size, texture_size), extended);
        band.write::<T>((0, 0), (texture_size, texture_size), &mut buffer)?;
    }

    Ok(())
}

mod cli;
mod dataset;
mod downsample;
mod fill_no_data;
pub mod gdal;
mod gdal_extension;
mod reproject;
mod result;
mod split;
mod stitch;

use crate::gdal::{
    Dataset, DriverManager,
    raster::{GdalDataType, GdalType, ResampleAlg},
};
use crate::{
    cli::PreprocessBar,
    dataset::{PreprocessContext, clear_directory, delete_directory},
    downsample::downsample_and_stitch,
    fill_no_data::create_mask_and_fill_no_data,
    reproject::{reproject, reproject_planar},
    split::split_and_stitch,
};
use bevy_terrain::prelude::{AttachmentLabel, TerrainConfig, TerrainShape, TileCoordinate};
use num::NumCast;
use std::time::Instant;

pub mod prelude {
    pub use crate::{
        cli::Cli,
        dataset::{PreprocessContext, PreprocessDataType, PreprocessNoData},
        preprocess,
    };
}

pub struct PreprocessData {
    pub dataset: Dataset,
    pub context: PreprocessContext,
}

fn preprocess_gen<T: Copy + GdalType + PartialEq + NumCast + Send + Sync>(
    src_dataset: &Dataset,
    context: &mut PreprocessContext,
) {
    if context.overwrite {
        clear_directory(&context.tile_dir);
    }

    clear_directory(&context.temp_dir);

    let start_preprocessing = Instant::now();

    let is_planar = matches!(context.shape, TerrainShape::Plane { .. });

    let progress_bar = PreprocessBar::new("Reprojecting".to_string());

    // Handle planar differently than spherical
    let faces = if is_planar {
        // For planar, just work with face 0 but ensure we generate a grid of tiles
        reproject_planar::<T>(src_dataset, context, Some(progress_bar.callback())).unwrap()
    } else {
        // Original spherical reprojection
        reproject::<T>(src_dataset, context, Some(progress_bar.callback())).unwrap()
    };

    progress_bar.finish();

    let progress_bar = PreprocessBar::new("Splitting".to_string());
    let tiles = split_and_stitch::<T>(faces, context, Some(progress_bar.callback())).unwrap();
    progress_bar.finish();

    let progress_bar = PreprocessBar::new("Downsampling".to_string());
    let tiles = downsample_and_stitch::<T>(&tiles, context, Some(progress_bar.callback())).unwrap();
    progress_bar.finish();

    let progress_bar = PreprocessBar::new("Filling".to_string());
    create_mask_and_fill_no_data(&tiles, context, Some(progress_bar.callback())).unwrap();
    progress_bar.finish();

    delete_directory(&context.temp_dir);

    save_terrain_config(tiles, context);

    println!("Preprocessing took: {:?}", start_preprocessing.elapsed());
}

pub fn preprocess(preprocess_data_list: &mut Vec<PreprocessData>) {
    preprocess_data_list
        .sort_by_key(|data| data.context.attachment_label != AttachmentLabel::Height);

    let mut target_width = 0;
    let mut target_height = 0;

    for preprocess_data in preprocess_data_list {
        let mut src_dataset = &preprocess_data.dataset;
        let mut context = &mut preprocess_data.context;

        if context.attachment_label == AttachmentLabel::Height {
            target_width = src_dataset.raster_size().0;
            target_height = src_dataset.raster_size().1;
        }
        // RESIZE OTHER MAPS TO MATCH HEIGHT MAP
        // This prevents spatial misalignment caused by different LOD domain calculations
        let (width, height) = src_dataset.raster_size();
        let resized: Dataset;
        src_dataset = if target_width != 0
            && target_height != 0
            && (width != target_width || height != target_height)
        {
            println!(
                "Resizing {} from {}x{} to {}x{} to match Height map...",
                context.attachment_label, width, height, target_width, target_height
            );

            let driver = DriverManager::get_driver_by_name("MEM").unwrap();
            resized = driver
                .create(
                    "",
                    target_width as usize,
                    target_height as usize,
                    src_dataset.raster_count() as usize,
                )
                .unwrap();

            // Copy GeoTransform (scaled)
            if let Ok(mut geo) = src_dataset.geo_transform() {
                geo[1] *= width as f64 / target_width as f64;
                geo[5] *= height as f64 / target_height as f64;
                resized.set_geo_transform(&geo).unwrap();
            }
            // Copy Projection
            let proj = src_dataset.projection();
            resized.set_projection(&proj).unwrap();

            for i in 1..=src_dataset.raster_count() {
                let src_band = src_dataset.rasterband(i).unwrap();
                let mut dst_band = resized.rasterband(i).unwrap();

                // Read entire image with resampling (u8 for Albedo)
                let mut buffer = src_band
                    .read_as::<u8>(
                        (0, 0),
                        (width, height),
                        (target_width, target_height),
                        Some(ResampleAlg::Bilinear),
                    )
                    .unwrap();

                dst_band
                    .write::<u8>((0, 0), (target_width, target_height), &mut buffer)
                    .unwrap();
            }
            &resized
        } else {
            src_dataset
        };
        macro_rules! preprocess_gen {
            ($data_type:ty) => {
                preprocess_gen::<$data_type>(src_dataset, &mut context)
            };
        }

        match context.data_type {
            GdalDataType::Unknown => panic!("Unknown data type!"),
            GdalDataType::UInt8 => preprocess_gen!(u8),
            GdalDataType::UInt16 => preprocess_gen!(u16),
            GdalDataType::UInt32 => preprocess_gen!(u32),
            GdalDataType::UInt64 => preprocess_gen!(u64),
            GdalDataType::Int16 => preprocess_gen!(i16),
            GdalDataType::Int32 => preprocess_gen!(i32),
            GdalDataType::Int64 => preprocess_gen!(i64),
            GdalDataType::Float32 => preprocess_gen!(f32),
            GdalDataType::Float64 => preprocess_gen!(f64),
        };
    }
}

fn save_terrain_config(tiles: Vec<TileCoordinate>, context: &PreprocessContext) {
    let file_path = context.terrain_path.join("config.tc.ron");

    let mut config = TerrainConfig::load_file(&file_path).unwrap_or_default();

    config.shape = context.shape;

    let path = context.terrain_path.to_str().unwrap().replace("\\", "/");
    config.path = if let Some(index) = path.rfind("assets/") {
        path[index..].to_string()
    } else {
        path
    };

    config.add_attachment(context.attachment_label.clone(), context.attachment.clone());

    if context.attachment_label == AttachmentLabel::Height {
        config.min_height = context.min_height;
        config.max_height = context.max_height;
        config.height_scale = context.height_scale;
        config.tiles = tiles;
        config.lod_count = context.lod_count.unwrap();
    }

    config.save_file(&file_path).unwrap();
}

use std::env::set_var;

use bevy_terrain::prelude::{AttachmentFormat, AttachmentLabel};
use bevy_terrain_preprocess::gdal::raster::GdalDataType;
use bevy_terrain_preprocess::{
    PreprocessData,
    prelude::{Cli, PreprocessContext, PreprocessDataType, PreprocessNoData, preprocess},
};

fn main() {
    unsafe {
        if true {
            set_var("RAYON_NUM_THREADS", "0");
            set_var("GDAL_NUM_THREADS", "ALL_CPUS");
        } else {
            set_var("RAYON_NUM_THREADS", "1");
            set_var("GDAL_NUM_THREADS", "1");
        }
    }

    let args1 = Cli {
        src_path: vec!["assets/source_data/gebco_earth.tif".into()],
        terrain_path: "../assets/terrains/earth_flat".into(),
        temp_path: None,
        overwrite: true,
        no_data: PreprocessNoData::Source,
        data_type: PreprocessDataType::DataType(GdalDataType::Float32),
        fill_radius: 16.0,
        create_mask: true,
        clip_to_source_extent: false,
        lod_count: None,
        attachment_label: AttachmentLabel::Height,
        texture_size: 512,
        border_size: 2,
        side_length: 4000000.,
        height_scale: 0.,
        radius: None,
        mip_level_count: 1,
        format: AttachmentFormat::R32F,
    };

    let args2 = Cli {
        src_path: vec!["assets/source_data/true_marble.tif".into()],
        terrain_path: "../assets/terrains/earth_flat".into(),
        temp_path: None,
        overwrite: true,
        no_data: PreprocessNoData::Source,
        data_type: PreprocessDataType::DataType(GdalDataType::UInt8),
        fill_radius: 16.0,
        create_mask: false,
        clip_to_source_extent: false,
        lod_count: None,
        attachment_label: AttachmentLabel::Custom("Albedo".to_string()),
        texture_size: 512,
        border_size: 2,
        side_length: 4000000.,
        height_scale: 0.,
        radius: None,
        mip_level_count: 1,
        format: AttachmentFormat::Rgba8U,
    };

    let mut data_list: Vec<PreprocessData> = [args1, args2]
        .into_iter()
        .map(|args| {
            let (dataset, context) = PreprocessContext::from_cli(args).unwrap();
            PreprocessData { dataset, context }
        })
        .collect();

    preprocess(&mut data_list);
}

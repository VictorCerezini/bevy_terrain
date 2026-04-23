use bevy_terrain::prelude::{AttachmentFormat, AttachmentLabel};
use bevy_terrain_preprocess::gdal::raster::GdalDataType;
use bevy_terrain_preprocess::{
    PreprocessData,
    prelude::{Cli, PreprocessContext, PreprocessDataType, PreprocessNoData, preprocess},
};

fn main() {
    let args = Cli {
        src_path: vec!["assets/source_data/morrowind.tif".into()],
        terrain_path: "../assets/terrains/swiss".into(),
        temp_path: None,
        overwrite: true,
        no_data: PreprocessNoData::Source,
        data_type: PreprocessDataType::DataType(GdalDataType::Float32),
        fill_radius: 32.0,
        create_mask: false,
        clip_to_source_extent: true,
        lod_count: None,
        attachment_label: AttachmentLabel::Height,
        texture_size: 512,
        side_length: 40000.,
        height_scale: 0.,
        radius: None,
        border_size: 2,
        mip_level_count: 2,
        format: AttachmentFormat::R32F,
    };

    let mut data_list: Vec<PreprocessData> = [args]
        .into_iter()
        .map(|args| {
            let (dataset, context) = PreprocessContext::from_cli(args).unwrap();
            PreprocessData { dataset, context }
        })
        .collect();

    preprocess(&mut data_list);
}

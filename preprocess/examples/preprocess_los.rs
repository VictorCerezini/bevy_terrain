use bevy_terrain::prelude::{AttachmentFormat, AttachmentLabel};
use bevy_terrain_preprocess::{PreprocessData, prelude::{
    Cli, PreprocessContext, PreprocessDataType, PreprocessNoData, preprocess,
}};
use gdal::raster::GdalDataType;

fn main() {
    let args = Cli {
        src_path: vec!["assets/source_data/LOS.tiff".into()],
        terrain_path: "../assets/terrains/los".into(),
        temp_path: None,
        overwrite: true,
        no_data: PreprocessNoData::Source,
        data_type: PreprocessDataType::DataType(GdalDataType::Float32),
        fill_radius: 32.0,
        create_mask: true,
        lod_count: None,
        attachment_label: AttachmentLabel::Height,
        texture_size: 512,
        border_size: 4,
        side_length: 86400000.,
        radius: Some(86400000.),
        mip_level_count: 1,
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

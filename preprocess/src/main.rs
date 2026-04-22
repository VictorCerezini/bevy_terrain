use bevy_terrain_preprocess::{
    PreprocessData,
    prelude::{Cli, PreprocessContext, preprocess},
};
use clap::Parser;
use std::env::set_var;

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

    let args = Cli::parse();

    let mut data_list: Vec<PreprocessData> = [args]
        .into_iter()
        .map(|args| {
            let (dataset, context) = PreprocessContext::from_cli(args).unwrap();
            PreprocessData { dataset, context }
        })
        .collect();

    preprocess(&mut data_list);
}

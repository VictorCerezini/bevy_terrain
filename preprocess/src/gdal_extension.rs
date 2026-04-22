use crate::{
    dataset::PreprocessContext,
    gdal::{
        Dataset, GeoTransform,
        raster::{GdalType, ResampleAlg},
    },
    result::{PreprocessError, PreprocessResult},
};
use bevy_math::U64Vec2;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use thread_local::ThreadLocal;

type CreateSimilarFunc = unsafe extern "C" fn(
    transformer_arg: *mut std::ffi::c_void,
    src_ratio_x: f64,
    src_ratio_y: f64,
) -> *mut std::ffi::c_void;

#[repr(C)]
pub struct GDALTransformerInfo {
    pfn_create_similar: Option<CreateSimilarFunc>,
}

impl GDALTransformerInfo {
    pub(crate) fn new(similar_func: CreateSimilarFunc) -> Self {
        Self {
            pfn_create_similar: Some(similar_func),
        }
    }
}

#[repr(C)]
pub struct GDALCustomTransformer {
    pub(crate) info: GDALTransformerInfo,
    pub(crate) inner: Box<dyn Transformer>,
}

pub fn warp<T: GdalType>(
    src: &Dataset,
    dst: &Dataset,
    context: &PreprocessContext,
    transformer: &mut GDALCustomTransformer,
    progress_callback: Option<&ProgressCallback>,
) -> PreprocessResult<()> {
    let (width, height) = dst.raster_size();
    let band_count = context.rasterbands.len();

    for band_index in 1..=band_count {
        let src_band = src.rasterband(band_index)?;
        let mut dst_band = dst.rasterband(band_index)?;
        let mut buffer = src_band.read_as::<T>(
            (0, 0),
            src.raster_size(),
            (width, height),
            Some(ResampleAlg::Bilinear),
        )?;
        dst_band.write::<T>((0, 0), (width, height), &mut buffer)?;
        if let Some(progress_callback) = progress_callback {
            progress_callback(band_index as f64 / band_count as f64);
        }
    }

    let _ = transformer;
    Ok(())
}

pub fn fill_no_data(src: &Dataset, fill_radius: f64) -> PreprocessResult<()> {
    if fill_radius <= 0.0 {
        return Ok(());
    }

    for band_index in 1..=src.raster_count() {
        let mut band = src.rasterband(band_index)?;
        let Some(no_data) = band.no_data_value() else {
            continue;
        };
        let (width, height) = src.raster_size();
        let mut buffer = band.read_band_as::<f64>()?;
        let original = buffer.data().to_vec();
        let radius = fill_radius.ceil() as isize;

        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                if original[index] != no_data {
                    continue;
                }

                let mut sum = 0.0;
                let mut count = 0.0;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let sx = x as isize + dx;
                        let sy = y as isize + dy;
                        if sx < 0 || sy < 0 || sx >= width as isize || sy >= height as isize {
                            continue;
                        }
                        let value = original[sy as usize * width + sx as usize];
                        if value != no_data && value.is_finite() {
                            sum += value;
                            count += 1.0;
                        }
                    }
                }
                if count > 0.0 {
                    buffer.data_mut()[index] = sum / count;
                }
            }
        }
        band.write::<f64>((0, 0), (width, height), &mut buffer)?;
    }

    Ok(())
}

pub type ProgressCallback<'a> = dyn Fn(f64) -> bool + Sync + 'a;

pub(crate) struct CountingProgressCallback<'a> {
    count: f64,
    counter: AtomicU64,
    progress_callback: Option<&'a ProgressCallback<'a>>,
}

impl<'a> CountingProgressCallback<'a> {
    pub(crate) fn new(count: u64, progress_callback: Option<&'a ProgressCallback<'a>>) -> Self {
        Self {
            count: count.max(1) as f64,
            counter: AtomicU64::new(1),
            progress_callback,
        }
    }

    pub(crate) fn increment(&self) {
        if let Some(progress_callback) = self.progress_callback {
            progress_callback(self.counter.fetch_add(1, Ordering::Relaxed) as f64 / self.count);
        }
    }
}

pub trait Transformer: Send + Sync {
    fn transform(
        &mut self,
        dst_to_src: bool,
        x: &mut [f64],
        y: &mut [f64],
        z: &mut [f64],
        success: &mut [bool],
    ) -> PreprocessResult<()>;
}

pub struct SuggestedWarpOutput {
    pub size: U64Vec2,
    pub geo_transform: GeoTransform,
}

impl SuggestedWarpOutput {
    pub fn compute(
        src: &Dataset,
        transformer: &mut GDALCustomTransformer,
    ) -> Result<Option<SuggestedWarpOutput>, PreprocessError> {
        let (width, height) = src.raster_size();
        let mut xs = vec![0.0, width as f64, 0.0, width as f64];
        let mut ys = vec![0.0, 0.0, height as f64, height as f64];
        let mut zs = vec![0.0; 4];
        let mut success = vec![true; 4];
        transformer
            .inner
            .transform(false, &mut xs, &mut ys, &mut zs, &mut success)?;
        if success.iter().all(|success| !success) {
            return Ok(None);
        }
        let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min).max(0.0);
        let max_x = xs
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            .min(1.0);
        let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min).max(0.0);
        let max_y = ys
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            .min(1.0);
        let src_pixels = (width.max(height)).max(1) as f64;
        let out_width = ((max_x - min_x).abs() * src_pixels).ceil().max(1.0) as u64;
        let out_height = ((max_y - min_y).abs() * src_pixels).ceil().max(1.0) as u64;

        Ok(Some(SuggestedWarpOutput {
            size: U64Vec2::new(out_width, out_height),
            geo_transform: [
                min_x,
                (max_x - min_x) / out_width as f64,
                0.0,
                min_y,
                0.0,
                (max_y - min_y) / out_height as f64,
            ],
        }))
    }
}

pub struct SharedReadOnlyDataset {
    path: std::path::PathBuf,
    pool: ThreadLocal<Dataset>,
}

impl SharedReadOnlyDataset {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            pool: ThreadLocal::new(),
        }
    }

    pub fn get(&self) -> &Dataset {
        self.pool.get_or(|| {
            Dataset::open(&self.path).unwrap_or_else(|err| {
                panic!(
                    "failed to open shared dataset {}: {err}",
                    self.path.display()
                )
            })
        })
    }
}

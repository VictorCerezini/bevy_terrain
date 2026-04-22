use crate::{
    dataset::PreprocessContext,
    gdal::{
        Dataset,
        raster::{GdalType, ResampleAlg},
    },
    result::PreprocessResult,
};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use thread_local::ThreadLocal;

pub fn warp<T: GdalType>(
    src: &Dataset,
    dst: &Dataset,
    context: &PreprocessContext,
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

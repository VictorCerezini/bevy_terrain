use crate::result::{PreprocessError, PreprocessResult};
use geotiff_reader::GeoTiffFile;
use ndarray::Array2;
use num::{NumCast, ToPrimitive};
use std::{
    fs::File,
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};
use tiff_core::{ExtraSample, PhotometricInterpretation, PlanarConfiguration, Tag, TagValue};
use tiff_reader::{TiffFile, TiffSample};
use tiff_writer::{ImageBuilder, TiffWriteSample, TiffWriter, WriteOptions};

pub mod errors {
    use thiserror::Error;

    #[derive(Error, Debug, Clone)]
    pub enum GdalError {
        #[error("{0}")]
        Message(String),
    }

    pub type Result<T> = std::result::Result<T, GdalError>;
}

pub mod spatial_ref {
    use super::errors::GdalError;

    #[derive(Debug, Clone)]
    pub struct SpatialRef {
        #[allow(dead_code)]
        pub(crate) proj4: String,
    }

    impl SpatialRef {
        pub fn from_proj4(proj4: &str) -> Result<Self, GdalError> {
            Ok(Self {
                proj4: proj4.to_string(),
            })
        }
    }
}

pub mod raster {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum GdalDataType {
        Unknown,
        UInt8,
        UInt16,
        UInt32,
        UInt64,
        Int16,
        Int32,
        Int64,
        Float32,
        Float64,
    }

    impl GdalDataType {
        pub fn from_name(name: &str) -> Result<Self, errors::GdalError> {
            match name.trim().to_ascii_lowercase().as_str() {
                "byte" | "uint8" | "u8" => Ok(Self::UInt8),
                "uint16" | "u16" => Ok(Self::UInt16),
                "uint32" | "u32" => Ok(Self::UInt32),
                "uint64" | "u64" => Ok(Self::UInt64),
                "int16" | "i16" => Ok(Self::Int16),
                "int32" | "i32" => Ok(Self::Int32),
                "int64" | "i64" => Ok(Self::Int64),
                "float32" | "f32" => Ok(Self::Float32),
                "float64" | "f64" => Ok(Self::Float64),
                other => Err(errors::GdalError::Message(format!(
                    "unknown raster data type: {other}"
                ))),
            }
        }
    }

    pub trait GdalType:
        Copy
        + Clone
        + Default
        + NumCast
        + ToPrimitive
        + TiffSample
        + TiffWriteSample
        + Send
        + Sync
        + 'static
    {
        const DATA_TYPE: GdalDataType;
    }

    macro_rules! impl_gdal_type {
        ($ty:ty, $data_type:expr) => {
            impl GdalType for $ty {
                const DATA_TYPE: GdalDataType = $data_type;
            }
        };
    }

    impl_gdal_type!(u8, GdalDataType::UInt8);
    impl_gdal_type!(u16, GdalDataType::UInt16);
    impl_gdal_type!(u32, GdalDataType::UInt32);
    impl_gdal_type!(u64, GdalDataType::UInt64);
    impl_gdal_type!(i16, GdalDataType::Int16);
    impl_gdal_type!(i32, GdalDataType::Int32);
    impl_gdal_type!(i64, GdalDataType::Int64);
    impl_gdal_type!(f32, GdalDataType::Float32);
    impl_gdal_type!(f64, GdalDataType::Float64);

    #[derive(Debug, Clone, Copy)]
    pub enum ColorInterpretation {
        Undefined,
        GrayIndex,
        RedBand,
        GreenBand,
        BlueBand,
        AlphaBand,
    }

    impl ColorInterpretation {
        pub fn c_int(self) -> i32 {
            match self {
                Self::Undefined => 0,
                Self::GrayIndex => 1,
                Self::RedBand => 3,
                Self::GreenBand => 4,
                Self::BlueBand => 5,
                Self::AlphaBand => 6,
            }
        }

        pub fn from_c_int(value: i32) -> Option<Self> {
            match value {
                1 => Some(Self::GrayIndex),
                3 => Some(Self::RedBand),
                4 => Some(Self::GreenBand),
                5 => Some(Self::BlueBand),
                6 => Some(Self::AlphaBand),
                _ => Some(Self::Undefined),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub enum ResampleAlg {
        NearestNeighbour,
        Bilinear,
    }

    pub struct RasterCreationOptions;

    impl RasterCreationOptions {
        pub fn from_iter<T>(_iter: T) -> Self
        where
            T: IntoIterator,
            T::Item: AsRef<str>,
        {
            Self
        }
    }

    #[derive(Debug, Clone)]
    pub struct Buffer<T> {
        shape: (usize, usize),
        data: Vec<T>,
    }

    impl<T> Buffer<T> {
        pub fn new(shape: (usize, usize), data: Vec<T>) -> Self {
            Self { shape, data }
        }

        pub fn data(&self) -> &[T] {
            &self.data
        }

        pub fn data_mut(&mut self) -> &mut [T] {
            &mut self.data
        }
    }

    impl<T: Clone> Buffer<T> {
        pub fn to_array(&self) -> PreprocessResult<Array2<T>> {
            Array2::from_shape_vec((self.shape.1, self.shape.0), self.data.clone())
                .map_err(|err| PreprocessError::Raster(err.to_string()))
        }
    }

    impl<T> From<Array2<T>> for Buffer<T> {
        fn from(array: Array2<T>) -> Self {
            let shape = (array.shape()[1], array.shape()[0]);
            let data = array.into_iter().collect();
            Self { shape, data }
        }
    }

    impl<T> std::ops::Index<(usize, usize)> for Buffer<T> {
        type Output = T;

        fn index(&self, index: (usize, usize)) -> &Self::Output {
            &self.data[index.0 * self.shape.0 + index.1]
        }
    }

    impl<T> std::ops::IndexMut<(usize, usize)> for Buffer<T> {
        fn index_mut(&mut self, index: (usize, usize)) -> &mut Self::Output {
            &mut self.data[index.0 * self.shape.0 + index.1]
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct RasterMinMax {
        pub min: f64,
        pub max: f64,
    }

    pub use super::RasterBand;
}

pub mod programs {
    pub mod raster {
        use crate::gdal::{Dataset, errors::GdalError};

        pub fn build_vrt(
            _path: Option<&std::path::Path>,
            datasets: &[Dataset],
            _options: Option<()>,
        ) -> Result<Dataset, GdalError> {
            datasets
                .first()
                .cloned()
                .ok_or_else(|| GdalError::Message("cannot build VRT from no datasets".to_string()))
        }
    }
}

use errors::GdalError;
pub use raster::{
    Buffer, ColorInterpretation, GdalDataType, GdalType, RasterCreationOptions, ResampleAlg,
};

pub type GeoTransform = [f64; 6];

fn tiff_io_lock() -> &'static RwLock<()> {
    static LOCK: OnceLock<RwLock<()>> = OnceLock::new();
    LOCK.get_or_init(|| RwLock::new(()))
}

pub trait GeoTransformEx {
    fn apply(&self, x: f64, y: f64) -> (f64, f64);
    fn invert(&self) -> Result<GeoTransform, GdalError>;
}

impl GeoTransformEx for GeoTransform {
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self[0] + x * self[1] + y * self[2],
            self[3] + x * self[4] + y * self[5],
        )
    }

    fn invert(&self) -> Result<GeoTransform, GdalError> {
        let det = self[1] * self[5] - self[2] * self[4];
        if det.abs() < f64::EPSILON {
            return Err(GdalError::Message(
                "geo transform is not invertible".to_string(),
            ));
        }
        let inv_det = 1.0 / det;
        let b = self[5] * inv_det;
        let c = -self[2] * inv_det;
        let e = -self[4] * inv_det;
        let f = self[1] * inv_det;
        let a = -(b * self[0] + c * self[3]);
        let d = -(e * self[0] + f * self[3]);
        Ok([a, b, c, d, e, f])
    }
}

#[derive(Default)]
pub struct DatasetOptions {
    pub open_flags: GdalOpenFlags,
}

#[derive(Default)]
#[allow(non_camel_case_types)]
pub enum GdalOpenFlags {
    #[default]
    ReadOnly,
    GDAL_OF_UPDATE,
}

#[derive(Clone)]
struct BandData {
    data: Vec<f64>,
    no_data: Option<f64>,
    color: ColorInterpretation,
}

struct DatasetData {
    path: Option<PathBuf>,
    width: usize,
    height: usize,
    data_type: GdalDataType,
    geo_transform: Option<GeoTransform>,
    projection: String,
    bands: Vec<BandData>,
    dirty: bool,
}

#[derive(Clone)]
pub struct Dataset {
    inner: Arc<RwLock<DatasetData>>,
}

pub struct Driver {
    name: String,
}

pub struct DriverManager;

impl DriverManager {
    pub fn get_driver_by_name(name: &str) -> Result<Driver, GdalError> {
        Ok(Driver {
            name: name.to_string(),
        })
    }
}

impl Driver {
    pub fn create(
        &self,
        path: &str,
        width: usize,
        height: usize,
        band_count: usize,
    ) -> Result<Dataset, GdalError> {
        self.create_with_band_type_with_options::<u8, _>(
            path,
            width,
            height,
            band_count,
            &RasterCreationOptions,
        )
    }

    pub fn create_with_band_type_with_options<T: GdalType, P: AsRef<Path>>(
        &self,
        path: P,
        width: usize,
        height: usize,
        band_count: usize,
        _options: &RasterCreationOptions,
    ) -> Result<Dataset, GdalError> {
        let path = path.as_ref();
        Ok(Dataset::empty(
            (self.name != "MEM").then(|| path.to_path_buf()),
            width,
            height,
            band_count,
            T::DATA_TYPE,
        ))
    }
}

impl Dataset {
    fn empty(
        path: Option<PathBuf>,
        width: usize,
        height: usize,
        band_count: usize,
        data_type: GdalDataType,
    ) -> Self {
        let bands = (0..band_count)
            .map(|index| BandData {
                data: vec![0.0; width * height],
                no_data: None,
                color: match index {
                    0 if band_count == 1 => ColorInterpretation::GrayIndex,
                    0 => ColorInterpretation::RedBand,
                    1 => ColorInterpretation::GreenBand,
                    2 => ColorInterpretation::BlueBand,
                    3 => ColorInterpretation::AlphaBand,
                    _ => ColorInterpretation::Undefined,
                },
            })
            .collect();
        Self {
            inner: Arc::new(RwLock::new(DatasetData {
                path,
                width,
                height,
                data_type,
                geo_transform: None,
                projection: String::new(),
                bands,
                dirty: true,
            })),
        }
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, GdalError> {
        Self::open_path(path.as_ref())
    }

    pub fn open_ex<P: AsRef<Path>>(path: P, _options: DatasetOptions) -> Result<Self, GdalError> {
        Self::open_path(path.as_ref())
    }

    fn open_path(path: &Path) -> Result<Self, GdalError> {
        let _io_guard = tiff_io_lock().read().unwrap();
        let bytes = std::fs::read(path)
            .map_err(|err| GdalError::Message(format!("{}: {err}", path.display())))?;
        if let Ok(file) = GeoTiffFile::from_bytes(bytes.clone()) {
            return Self::from_tiff(
                path,
                file.tiff(),
                file.transform().map(|t| {
                    [
                        t.origin_x,
                        t.pixel_width,
                        t.skew_x,
                        t.origin_y,
                        t.skew_y,
                        t.pixel_height,
                    ]
                }),
                file.nodata().and_then(|v| v.parse().ok()),
            );
        }

        let file = TiffFile::from_bytes(bytes)
            .map_err(|err| GdalError::Message(format!("{}: {err}", path.display())))?;
        Self::from_tiff(path, &file, None, None)
    }

    fn from_tiff(
        path: &Path,
        file: &TiffFile,
        geo_transform: Option<GeoTransform>,
        no_data: Option<f64>,
    ) -> Result<Self, GdalError> {
        let ifd = file
            .ifd(0)
            .map_err(|err| GdalError::Message(err.to_string()))?;
        let width = ifd.width() as usize;
        let height = ifd.height() as usize;
        let samples = ifd.samples_per_pixel() as usize;
        let layout = ifd
            .raster_layout()
            .map_err(|err| GdalError::Message(err.to_string()))?;
        let data_type = match (layout.sample_format, layout.bits_per_sample) {
            (1, 8) => GdalDataType::UInt8,
            (1, 16) => GdalDataType::UInt16,
            (1, 32) => GdalDataType::UInt32,
            (1, 64) => GdalDataType::UInt64,
            (2, 16) => GdalDataType::Int16,
            (2, 32) => GdalDataType::Int32,
            (2, 64) => GdalDataType::Int64,
            (3, 32) => GdalDataType::Float32,
            (3, 64) => GdalDataType::Float64,
            _ => GdalDataType::Unknown,
        };

        let bands = match data_type {
            GdalDataType::UInt8 => read_bands::<u8>(file, samples, no_data)?,
            GdalDataType::UInt16 => read_bands::<u16>(file, samples, no_data)?,
            GdalDataType::UInt32 => read_bands::<u32>(file, samples, no_data)?,
            GdalDataType::UInt64 => read_bands::<u64>(file, samples, no_data)?,
            GdalDataType::Int16 => read_bands::<i16>(file, samples, no_data)?,
            GdalDataType::Int32 => read_bands::<i32>(file, samples, no_data)?,
            GdalDataType::Int64 => read_bands::<i64>(file, samples, no_data)?,
            GdalDataType::Float32 => read_bands::<f32>(file, samples, no_data)?,
            GdalDataType::Float64 => read_bands::<f64>(file, samples, no_data)?,
            GdalDataType::Unknown => {
                return Err(GdalError::Message(
                    "unsupported TIFF sample type".to_string(),
                ));
            }
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(DatasetData {
                path: Some(path.to_path_buf()),
                width,
                height,
                data_type,
                geo_transform,
                projection: String::new(),
                bands,
                dirty: false,
            })),
        })
    }

    pub fn raster_size(&self) -> (usize, usize) {
        let inner = self.inner.read().unwrap();
        (inner.width, inner.height)
    }

    pub fn raster_count(&self) -> usize {
        self.inner.read().unwrap().bands.len()
    }

    pub fn rasterband(&self, index: usize) -> Result<RasterBand<'_>, GdalError> {
        if index == 0 || index > self.raster_count() {
            return Err(GdalError::Message(format!(
                "raster band {index} out of range"
            )));
        }
        Ok(RasterBand {
            dataset: self.clone(),
            index: index - 1,
            mask: false,
            _marker: PhantomData,
        })
    }

    pub fn rasterbands(&self) -> impl Iterator<Item = Result<RasterBand<'_>, GdalError>> + '_ {
        (1..=self.raster_count()).map(|index| self.rasterband(index))
    }

    pub fn geo_transform(&self) -> Result<GeoTransform, GdalError> {
        self.inner
            .read()
            .unwrap()
            .geo_transform
            .ok_or_else(|| GdalError::Message("dataset has no geo transform".to_string()))
    }

    pub fn set_geo_transform(&self, geo_transform: &GeoTransform) -> Result<(), GdalError> {
        let mut inner = self.inner.write().unwrap();
        inner.geo_transform = Some(*geo_transform);
        inner.dirty = true;
        Ok(())
    }

    pub fn projection(&self) -> String {
        self.inner.read().unwrap().projection.clone()
    }

    pub fn set_projection(&self, projection: &str) -> Result<(), GdalError> {
        let mut inner = self.inner.write().unwrap();
        inner.projection = projection.to_string();
        inner.dirty = true;
        Ok(())
    }

    pub fn spatial_ref(&self) -> Result<spatial_ref::SpatialRef, GdalError> {
        Ok(spatial_ref::SpatialRef {
            proj4: self.projection(),
        })
    }

    pub fn flush(&self) -> Result<(), GdalError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.dirty {
            return Ok(());
        }
        let Some(path) = &inner.path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| GdalError::Message(err.to_string()))?;
        }
        let result = match inner.data_type {
            GdalDataType::UInt8 => write_dataset::<u8>(&inner, path),
            GdalDataType::UInt16 => write_dataset::<u16>(&inner, path),
            GdalDataType::UInt32 => write_dataset::<u32>(&inner, path),
            GdalDataType::UInt64 => write_dataset::<u64>(&inner, path),
            GdalDataType::Int16 => write_dataset::<i16>(&inner, path),
            GdalDataType::Int32 => write_dataset::<i32>(&inner, path),
            GdalDataType::Int64 => write_dataset::<i64>(&inner, path),
            GdalDataType::Float32 => write_dataset::<f32>(&inner, path),
            GdalDataType::Float64 => write_dataset::<f64>(&inner, path),
            GdalDataType::Unknown => Ok(()),
        };
        if result.is_ok() {
            inner.dirty = false;
        }
        result
    }
}

impl Drop for Dataset {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.flush();
        }
    }
}

fn read_bands<T: GdalType>(
    file: &TiffFile,
    samples: usize,
    no_data: Option<f64>,
) -> Result<Vec<BandData>, GdalError> {
    let image = file
        .read_image::<T>(0)
        .map_err(|err| GdalError::Message(err.to_string()))?;
    let shape = image.shape().to_vec();
    let (values, offset) = image.into_raw_vec_and_offset();
    debug_assert_eq!(offset, Some(0));
    let pixels = shape[0] * shape[1];
    let mut bands = (0..samples)
        .map(|index| BandData {
            data: vec![0.0; pixels],
            no_data,
            color: match index {
                0 if samples == 1 => ColorInterpretation::GrayIndex,
                0 => ColorInterpretation::RedBand,
                1 => ColorInterpretation::GreenBand,
                2 => ColorInterpretation::BlueBand,
                3 => ColorInterpretation::AlphaBand,
                _ => ColorInterpretation::Undefined,
            },
        })
        .collect::<Vec<_>>();

    if samples == 1 {
        for (dst, value) in bands[0].data.iter_mut().zip(values) {
            *dst = value.to_f64().unwrap_or(0.0);
        }
    } else {
        for pixel in 0..pixels {
            for band in 0..samples {
                bands[band].data[pixel] = values[pixel * samples + band].to_f64().unwrap_or(0.0);
            }
        }
    }
    Ok(bands)
}

fn write_dataset<T: GdalType>(inner: &DatasetData, path: &Path) -> Result<(), GdalError> {
    let _io_guard = tiff_io_lock().write().unwrap();
    let mut file = File::create(path).map_err(|err| GdalError::Message(err.to_string()))?;
    let mut writer = TiffWriter::new(&mut file, WriteOptions::default())
        .map_err(|err| GdalError::Message(err.to_string()))?;
    let spp = inner.bands.len();
    let mut image = ImageBuilder::new(inner.width as u32, inner.height as u32)
        .sample_type::<T>()
        .samples_per_pixel(spp as u16)
        .planar_configuration(PlanarConfiguration::Chunky)
        .strips(inner.height as u32)
        .photometric(if spp >= 3 {
            PhotometricInterpretation::Rgb
        } else {
            PhotometricInterpretation::MinIsBlack
        });
    if spp == 4 {
        image = image.extra_samples(vec![ExtraSample::UnassociatedAlpha]);
    }
    if let Some(no_data) = inner.bands.first().and_then(|band| band.no_data) {
        image = image.tag(Tag::new(42113, TagValue::Ascii(no_data.to_string())));
    }
    if let Some(geo) = inner.geo_transform {
        if geo[2].abs() <= f64::EPSILON && geo[4].abs() <= f64::EPSILON {
            image = image
                .tag(Tag::new(
                    33922,
                    TagValue::Double(vec![0.0, 0.0, 0.0, geo[0], geo[3], 0.0]),
                ))
                .tag(Tag::new(
                    33550,
                    TagValue::Double(vec![geo[1], -geo[5], 0.0]),
                ));
        } else {
            image = image.tag(Tag::new(
                34264,
                TagValue::Double(vec![
                    geo[1], geo[2], 0.0, geo[0], geo[4], geo[5], 0.0, geo[3], 0.0, 0.0, 0.0, 0.0,
                    0.0, 0.0, 0.0, 1.0,
                ]),
            ));
        }
    }
    let handle = writer
        .add_image(image)
        .map_err(|err| GdalError::Message(err.to_string()))?;

    let mut interleaved = Vec::with_capacity(inner.width * inner.height * spp);
    for pixel in 0..inner.width * inner.height {
        for band in &inner.bands {
            interleaved.push(T::from(band.data[pixel]).unwrap_or_default());
        }
    }
    writer
        .write_block(&handle, 0, &interleaved)
        .map_err(|err| GdalError::Message(err.to_string()))?;
    writer
        .finish()
        .map_err(|err| GdalError::Message(err.to_string()))?;
    Ok(())
}

pub struct RasterBand<'a> {
    dataset: Dataset,
    index: usize,
    mask: bool,
    _marker: PhantomData<&'a ()>,
}

impl RasterBand<'_> {
    pub fn band_type(&self) -> GdalDataType {
        if self.mask {
            return GdalDataType::UInt8;
        }
        self.dataset.inner.read().unwrap().data_type
    }

    pub fn color_interpretation(&self) -> ColorInterpretation {
        if self.mask {
            return ColorInterpretation::GrayIndex;
        }
        self.dataset.inner.read().unwrap().bands[self.index].color
    }

    pub fn set_color_interpretation(
        &mut self,
        color: ColorInterpretation,
    ) -> Result<(), GdalError> {
        let mut inner = self.dataset.inner.write().unwrap();
        inner.bands[self.index].color = color;
        inner.dirty = true;
        Ok(())
    }

    pub fn no_data_value(&self) -> Option<f64> {
        self.dataset.inner.read().unwrap().bands[self.index].no_data
    }

    pub fn set_no_data_value(&mut self, value: Option<f64>) -> Result<(), GdalError> {
        let mut inner = self.dataset.inner.write().unwrap();
        inner.bands[self.index].no_data = value;
        inner.dirty = true;
        Ok(())
    }

    pub fn read_band_as<T: GdalType>(&self) -> Result<Buffer<T>, GdalError> {
        let (width, height) = self.dataset.raster_size();
        self.read_as((0, 0), (width, height), (width, height), None)
    }

    pub fn open_mask_band(&self) -> Result<RasterBand<'_>, GdalError> {
        Ok(RasterBand {
            dataset: self.dataset.clone(),
            index: self.index,
            mask: true,
            _marker: PhantomData,
        })
    }

    pub fn read_as<T: GdalType>(
        &self,
        offset: (isize, isize),
        window_size: (usize, usize),
        buffer_size: (usize, usize),
        resample_alg: Option<ResampleAlg>,
    ) -> Result<Buffer<T>, GdalError> {
        let inner = self.dataset.inner.read().unwrap();
        let band = &inner.bands[self.index];
        let mut output = Vec::with_capacity(buffer_size.0 * buffer_size.1);
        for y in 0..buffer_size.1 {
            for x in 0..buffer_size.0 {
                let src_x = sample_coord(x, buffer_size.0, window_size.0, offset.0, resample_alg);
                let src_y = sample_coord(y, buffer_size.1, window_size.1, offset.1, resample_alg);
                let value = if src_x < 0
                    || src_y < 0
                    || src_x >= inner.width as isize
                    || src_y >= inner.height as isize
                {
                    if self.mask {
                        0.0
                    } else {
                        band.no_data.unwrap_or(0.0)
                    }
                } else {
                    let value = band.data[src_y as usize * inner.width + src_x as usize];
                    if self.mask {
                        if band.no_data.is_some_and(|no_data| value == no_data) {
                            0.0
                        } else {
                            255.0
                        }
                    } else {
                        value
                    }
                };
                output.push(T::from(value).unwrap_or_default());
            }
        }
        Ok(Buffer::new(buffer_size, output))
    }

    pub fn write<T: GdalType>(
        &mut self,
        offset: (isize, isize),
        size: (usize, usize),
        buffer: &mut Buffer<T>,
    ) -> Result<(), GdalError> {
        let mut inner = self.dataset.inner.write().unwrap();
        let width = inner.width;
        let height = inner.height;
        let band = &mut inner.bands[self.index];
        for y in 0..size.1 {
            for x in 0..size.0 {
                let dst_x = offset.0 + x as isize;
                let dst_y = offset.1 + y as isize;
                if dst_x >= 0 && dst_y >= 0 && dst_x < width as isize && dst_y < height as isize {
                    let value = if buffer.data().len() == 1 {
                        buffer.data()[0]
                    } else {
                        buffer[(y, x)]
                    };
                    band.data[dst_y as usize * width + dst_x as usize] =
                        value.to_f64().unwrap_or(0.0);
                }
            }
        }
        inner.dirty = true;
        Ok(())
    }

    pub fn compute_raster_min_max(
        &self,
        _approx_ok: bool,
    ) -> Result<raster::RasterMinMax, GdalError> {
        let inner = self.dataset.inner.read().unwrap();
        let band = &inner.bands[self.index];
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &value in &band.data {
            if band.no_data != Some(value) && value.is_finite() {
                min = min.min(value);
                max = max.max(value);
            }
        }
        Ok(raster::RasterMinMax { min, max })
    }
}

fn sample_coord(
    dst: usize,
    dst_len: usize,
    src_len: usize,
    offset: isize,
    resample_alg: Option<ResampleAlg>,
) -> isize {
    if dst_len == src_len {
        return offset + dst as isize;
    }
    let scale = src_len as f64 / dst_len as f64;
    let src = (dst as f64 + 0.5) * scale - 0.5;
    let src = match resample_alg {
        Some(ResampleAlg::Bilinear) => src.round(),
        _ => src.floor(),
    };
    offset + src as isize
}

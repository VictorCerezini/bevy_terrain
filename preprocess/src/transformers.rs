use crate::{
    gdal::{Dataset, GeoTransform, GeoTransformEx, spatial_ref::SpatialRef},
    gdal_extension::{GDALCustomTransformer, GDALTransformerInfo, Transformer},
    result::PreprocessResult,
};
use bevy_math::{DVec2, DVec3};
use bevy_terrain::math::Coordinate;
use itertools::izip;
use std::ffi::c_void;

impl Transformer for GeoTransform {
    fn transform(
        &mut self,
        dst_to_src: bool,
        x: &mut [f64],
        y: &mut [f64],
        _: &mut [f64],
        _: &mut [bool],
    ) -> PreprocessResult<()> {
        let transform = if dst_to_src { *self } else { self.invert()? };

        for (x, y) in x.iter_mut().zip(y.iter_mut()) {
            (*x, *y) = transform.apply(*x, *y);
        }

        Ok(())
    }
}

pub struct ReprojectionTransformer;

impl ReprojectionTransformer {
    fn new(_src_spatial_ref: &SpatialRef, _dst_spatial_ref: &SpatialRef) -> PreprocessResult<Self> {
        Ok(Self)
    }
}

impl Transformer for ReprojectionTransformer {
    fn transform(
        &mut self,
        _dst_to_src: bool,
        _x: &mut [f64],
        _y: &mut [f64],
        _z: &mut [f64],
        _success: &mut [bool],
    ) -> PreprocessResult<()> {
        Ok(())
    }
}

struct CubeTransformer {
    face: u32,
}

impl CubeTransformer {
    fn new(face: u32) -> Self {
        Self { face }
    }
}

impl Transformer for CubeTransformer {
    fn transform(
        &mut self,
        dst_to_src: bool,
        lon_or_u: &mut [f64],
        lat_or_v: &mut [f64],
        _: &mut [f64],
        success: &mut [bool],
    ) -> PreprocessResult<()> {
        if dst_to_src {
            for (lon_or_u, lat_or_v, success) in
                izip!(lon_or_u.iter_mut(), lat_or_v.iter_mut(), success.iter_mut())
            {
                let coordinate = Coordinate::new(self.face, DVec2::new(*lon_or_u, *lat_or_v));
                let unit_position = coordinate.unit_position(true);

                let lon = unit_position.z.atan2(-unit_position.x);
                let lat = unit_position.y.asin();

                *success = *success && !lat.is_nan();
                *lon_or_u = lon.to_degrees();
                *lat_or_v = lat.to_degrees();
            }
        } else {
            for (lon_or_u, lat_or_v, success) in
                izip!(lon_or_u.iter_mut(), lat_or_v.iter_mut(), success.iter_mut())
            {
                let lon = lon_or_u.to_radians();
                let lat = lat_or_v.to_radians();

                let unit_position =
                    DVec3::new(-lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());

                let coordinate = Coordinate::from_unit_position(unit_position, true);

                *success = *success
                    && (unit_position.length() - 1.0).abs() < 0.00001
                    && coordinate.face == self.face;
                *lon_or_u = coordinate.uv.x;
                *lat_or_v = coordinate.uv.y;
            }
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn clone_custom_transformer(arg: *mut c_void, _: f64, _: f64) -> *mut c_void {
    arg
}

pub struct CustomTransformer {
    src_inverse_geo_transform: GeoTransform,
    dst_geo_transform: Option<GeoTransform>,
    lon_lat_transformer: ReprojectionTransformer,
    cube_transformer: CubeTransformer,
}

impl CustomTransformer {
    pub fn create(
        src: &Dataset,
        face: u32,
        dst_geo_transform: Option<GeoTransform>,
    ) -> PreprocessResult<GDALCustomTransformer> {
        let src_geo_transform = src.geo_transform().unwrap_or([
            0.0,
            360.0 / src.raster_size().0 as f64,
            0.0,
            90.0,
            0.0,
            -180.0 / src.raster_size().1 as f64,
        ]);
        Ok(GDALCustomTransformer {
            info: GDALTransformerInfo::new(clone_custom_transformer),
            inner: Box::new(Self {
                src_inverse_geo_transform: src_geo_transform.invert()?,
                dst_geo_transform,
                lon_lat_transformer: ReprojectionTransformer::new(
                    &src.spatial_ref()?,
                    &SpatialRef::from_proj4("+proj=lonlat +ellps=WGS84 +datum=WGS84")?,
                )?,
                cube_transformer: CubeTransformer::new(face),
            }),
        })
    }
}

impl Transformer for CustomTransformer {
    fn transform(
        &mut self,
        dst_to_src: bool,
        x: &mut [f64],
        y: &mut [f64],
        z: &mut [f64],
        success: &mut [bool],
    ) -> PreprocessResult<()> {
        for success in success.iter_mut() {
            *success = true;
        }

        if dst_to_src {
            if let Some(mut geo_transform) = self.dst_geo_transform {
                geo_transform.transform(dst_to_src, x, y, z, success)?;
            }

            self.cube_transformer
                .transform(dst_to_src, x, y, z, success)?;
            self.lon_lat_transformer
                .transform(dst_to_src, x, y, z, success)?;
            self.src_inverse_geo_transform
                .transform(dst_to_src, x, y, z, success)?;
        } else {
            self.src_inverse_geo_transform
                .transform(dst_to_src, x, y, z, success)?;
            self.lon_lat_transformer
                .transform(dst_to_src, x, y, z, success)?;
            self.cube_transformer
                .transform(dst_to_src, x, y, z, success)?;
        }

        Ok(())
    }
}

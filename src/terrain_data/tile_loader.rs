use crate::terrain_data::{AttachmentData, AttachmentFormat, AttachmentTile, TileAtlas};
use bevy::{
    asset::{AssetServer, Assets, Handle},
    image::Image,
    prelude::*,
    render::render_resource::TextureFormat,
};
use slab::Slab;

struct LoadingTile {
    handle: Handle<Image>,
    tile: AttachmentTile,
    format: AttachmentFormat,
}

#[derive(Component)]
pub struct DefaultLoader {
    loading_tiles: Slab<LoadingTile>,
}

impl Default for DefaultLoader {
    fn default() -> Self {
        Self {
            loading_tiles: Slab::with_capacity(32),
        }
    }
}

impl DefaultLoader {
    fn to_load_next(&self, tiles: &mut Vec<AttachmentTile>) -> Option<AttachmentTile> {
        // Todo: tile prioritization goes here
        tiles.pop()
    }

    fn finish_loading(
        &mut self,
        atlas: &mut TileAtlas,
        asset_server: &mut AssetServer,
        images: &mut Assets<Image>,
    ) {
        self.loading_tiles.retain(|_, tile| {
            if asset_server.is_loaded(tile.handle.id()) {
                let image = images.get(tile.handle.id()).unwrap();
                let bytes = image.data.as_ref().unwrap();

                let data = if image.texture_descriptor.format == TextureFormat::R8Unorm && tile.format == AttachmentFormat::Rgba8U {
                    let mut new_data = Vec::with_capacity(bytes.len() * 4);
                    for &b in bytes {
                        new_data.extend_from_slice(&[b, b, b, 255]);
                    }
                    AttachmentData::from_bytes(&new_data, tile.format)
                } else if (image.texture_descriptor.format == TextureFormat::Rgba8Unorm
                    || image.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb)
                    && tile.format == AttachmentFormat::Rgba8U
                    && bytes.len()
                        == (image.texture_descriptor.size.width
                            * image.texture_descriptor.size.height
                            * 3) as usize
                {
                    let mut new_data = Vec::with_capacity(bytes.len() / 3 * 4);
                    for chunk in bytes.chunks(3) {
                        new_data.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
                    }
                    AttachmentData::from_bytes(&new_data, tile.format)
                } else {
                    AttachmentData::from_bytes(bytes, tile.format)
                };

                atlas.tile_loaded(tile.tile.clone(), data);

                false
            } else {
                !asset_server.load_state(tile.handle.id()).is_failed()
            }
        });
    }

    fn start_loading(&mut self, atlas: &mut TileAtlas, asset_server: &mut AssetServer) {
        while self.loading_tiles.len() < self.loading_tiles.capacity() {
            if let Some(tile) = self.to_load_next(&mut atlas.to_load) {
                let attachment = &atlas.attachments[&tile.label];

                let path = tile
                    .coordinate
                    .path(&attachment.path.join(String::from(&tile.label)))
                    .to_string_lossy()
                    .replace('\\', "/");

                if !std::path::Path::new("assets").join(&path).exists() {
                    warn!("Missing terrain tile: assets/{}", path);
                    let data = AttachmentData::new_default(attachment.format, attachment.texture_size);
                    atlas.tile_loaded(tile.clone(), data);
                    continue;
                }

                self.loading_tiles.insert(LoadingTile {
                    handle: asset_server.load(path),
                    tile,
                    format: attachment.format,
                });
            } else {
                break;
            }
        }
    }
}

pub fn finish_loading(
    mut terrains: Query<(&mut TileAtlas, &mut DefaultLoader)>,
    mut asset_server: ResMut<AssetServer>,
    mut images: ResMut<Assets<Image>>,
) {
    for (mut tile_atlas, mut loader) in &mut terrains {
        loader.finish_loading(&mut tile_atlas, &mut asset_server, &mut images);
    }
}

pub fn start_loading(
    mut terrains: Query<(&mut TileAtlas, &mut DefaultLoader)>,
    mut asset_server: ResMut<AssetServer>,
) {
    for (mut tile_atlas, mut loader) in &mut terrains {
        loader.start_loading(&mut tile_atlas, &mut asset_server);
    }
}

pub mod archive_index;
pub mod authoring;
pub mod disc_metadata;
pub mod graphics;
pub mod install_data;
pub mod source;
pub mod write_plan;
pub mod zip_patch;

use sha2::{Digest, Sha256};
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub mod code;
pub mod executable;
pub mod menu;
pub mod menu_text;
pub mod prepare;
pub mod story;
pub mod story_font;
pub mod story_text;
pub mod text;

mod font_glyphs;
mod hinting;

pub mod snc;

pub mod graphic_archives;

pub mod label_graphics;

pub mod panel_graphics;

pub mod scope;

pub mod display_text;

pub mod gim_text;

pub mod pins;

pub mod translation_batches;
pub mod translation_edit;

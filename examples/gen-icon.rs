//! Regenerates `assets/icon.ico` from the app's icon design.
//!
//! The icon is drawn by `steam_switcher::tray::render_rgba` — the same code
//! the tray uses at runtime, so the file and the tray can never drift. Run
//! after changing the design:
//!
//! ```text
//! cargo run --example gen-icon
//! ```
//!
//! Each size is rendered with anti-aliasing, PNG-encoded, and packed into a
//! multi-size ICO container (PNG entries are supported by Windows Vista+).

use std::fs;
use std::io::Cursor;
use std::path::Path;

/// Sizes to bake into the .ico (Windows standard set).
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> anyhow::Result<()> {
    let pngs: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&size| {
            let rgba = steam_switcher::tray::render_rgba(size);
            encode_png(&rgba, size, size)
        })
        .collect();

    let ico = assemble_ico(&SIZES, &pngs)?;
    let out = Path::new("assets/icon.ico");
    fs::create_dir_all(out.parent().expect("icon path has a parent"))?;
    fs::write(out, &ico)?;
    println!(
        "wrote {} ({} bytes, sizes {:?})",
        out.display(),
        ico.len(),
        SIZES
    );
    Ok(())
}

/// Encode straight-alpha RGBA8 pixels as a PNG blob.
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut out), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png data");
    }
    out
}

/// Pack PNG blobs into an ICO container (ICONDIR + ICONDIRENTRYs + blobs).
fn assemble_ico(sizes: &[u32], pngs: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    assert_eq!(sizes.len(), pngs.len());

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes()); // image count

    // Directory entries start right after the 6-byte header, 16 bytes each.
    let mut offset = (6 + 16 * sizes.len()) as u32;
    for (&size, png) in sizes.iter().zip(pngs) {
        // 0 in the width/height bytes means 256.
        ico.push(if size == 256 { 0 } else { size as u8 });
        ico.push(if size == 256 { 0 } else { size as u8 });
        ico.push(0); // palette color count
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in pngs {
        ico.extend_from_slice(png);
    }
    Ok(ico)
}

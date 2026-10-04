//! Bounded stdin/stdout adapters used by temporary analysis tools.
use anyhow::{Context, Result, ensure};
use std::io::{Read, Write};

pub(super) fn archive_index() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin().take(1024 * 1024).read_to_end(&mut input)?;
    println!(
        "{}",
        serde_json::to_string(&puyo20_tool::archive_index::Index::parse(&input)?)?
    );
    Ok(())
}

pub(super) fn text_pair() -> Result<()> {
    let mut packet = Vec::new();
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut packet)?;
    ensure!(packet.len() <= 32 * 1024 * 1024, "text packet too large");
    let n = u32::from_le_bytes(puyo20_tool::graphics::bytes(&packet, 0, 4)?.try_into()?) as usize;
    let font = puyo20_tool::graphics::bytes(&packet, 4, n)?;
    let text = packet.get(4 + n..).context("missing MTX packet")?;
    println!("{}", puyo20_tool::text::inspect_pair(font, text)?);
    Ok(())
}

pub(super) fn graphics(operation: &str) -> Result<()> {
    use puyo20_tool::graphics;
    let mut input = Vec::new();
    std::io::stdin()
        .take(256 * 1024 * 1024)
        .read_to_end(&mut input)?;
    let mut output = std::io::stdout().lock();
    match operation {
        "decode" => {
            let im = graphics::decode(&input)?;
            for n in [im.width, im.height, im.format, im.order] {
                output.write_all(&(n as u32).to_le_bytes())?;
            }
            output.write_all(&im.rgba)?;
        }
        "encode" => {
            ensure!(input.len() >= 12, "short encode packet");
            let word = |i| u32::from_le_bytes(input[i..i + 4].try_into().unwrap()) as usize;
            let n = word(0);
            let w = word(4);
            let h = word(8);
            let original = graphics::bytes(&input, 12, n)?;
            let rgba = input.get(12 + n..).context("short RGBA packet")?;
            output.write_all(&graphics::encode(original, w, h, rgba)?)?;
        }
        "snt-table" => output.write_all(&serde_json::to_vec(&graphics::snt_table(&input)?)?)?,
        _ => unreachable!(),
    }
    Ok(())
}

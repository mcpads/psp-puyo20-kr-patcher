// PSP DXT5 stores color selectors first and alpha endpoints last.
// Decode arithmetic matches the inspected PPSSPP TextureDecoder.cpp path.
pub(super) fn decode(data: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut rgba = vec![0; width * height * 4];
    for by in 0..height / 4 {
        for bx in 0..width / 4 {
            let offset = (by * (width / 4) + bx) * 16;
            let b = &data[offset..offset + 16];
            let endpoints = [
                u16::from_le_bytes([b[4], b[5]]),
                u16::from_le_bytes([b[6], b[7]]),
            ];
            let mut colors = [[0u8; 3]; 4];
            for i in 0..2 {
                let v = endpoints[i];
                colors[i] = [
                    ((v >> 8) & 248) as u8,
                    ((v >> 3) & 252) as u8,
                    ((v << 3) & 248) as u8,
                ];
            }
            for (c, (a, z)) in colors[0].into_iter().zip(colors[1]).enumerate() {
                let a = u16::from(a);
                let z = u16::from(z);
                if endpoints[0] > endpoints[1] {
                    colors[2][c] = ((2 * a + z) / 3) as u8;
                    colors[3][c] = ((a + 2 * z) / 3) as u8;
                } else {
                    colors[2][c] = ((a + z) / 2) as u8;
                }
            }
            let mut alpha = [0u8; 8];
            alpha[0] = b[14];
            alpha[1] = b[15];
            let divisor = if alpha[0] > alpha[1] { 7u32 } else { 5 };
            for n in 1..divisor {
                let a = u32::from(alpha[0]) * ((divisor - n) * 256) / divisor;
                let z = u32::from(alpha[1]) * (n * 256) / divisor;
                alpha[n as usize + 1] = ((a + z + 31) >> 8) as u8;
            }
            if divisor == 5 {
                alpha[7] = 255;
            }
            let indices = u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], 0, 0]);
            for y in 0..4 {
                for x in 0..4 {
                    let color = colors[((b[y] >> (2 * x)) & 3) as usize];
                    let a = alpha[((indices >> (3 * (4 * y + x))) & 7) as usize];
                    let dest = ((by * 4 + y) * width + bx * 4 + x) * 4;
                    rgba[dest..dest + 3].copy_from_slice(&color);
                    rgba[dest + 3] = a;
                }
            }
        }
    }
    rgba
}

// BC3 uses alpha endpoints/selectors followed by color endpoints/selectors.
// PSP stores color selectors/endpoints followed by alpha selectors/endpoints.
pub(super) fn compress_block(pixels: [[u8; 4]; 16]) -> [u8; 16] {
    let mut bc3 = [0; 16];
    texpresso::Format::Bc3.compress_block_masked(
        pixels,
        0xffff,
        texpresso::Params {
            algorithm: texpresso::Algorithm::IterativeClusterFit,
            weigh_colour_by_alpha: true,
            ..Default::default()
        },
        &mut bc3,
    );
    let mut psp = [0; 16];
    psp[..4].copy_from_slice(&bc3[12..16]);
    psp[4..8].copy_from_slice(&bc3[8..12]);
    psp[8..14].copy_from_slice(&bc3[2..8]);
    psp[14..16].copy_from_slice(&bc3[..2]);
    psp
}

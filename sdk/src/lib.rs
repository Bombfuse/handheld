#![no_std]

// Memory map addresses
pub const SYSTEM_BASE: usize = 0x00000;
pub const SYSTEM_SIZE: usize = 512;

pub const INPUT_BASE: usize = 0x00200;
pub const INPUT_SIZE: usize = 48;

pub const FRAMEBUFFER_BASE: usize = 0x00300;
pub const SCREEN_WIDTH: usize = 320;
pub const SCREEN_HEIGHT: usize = 240;
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

pub const SAVE_BASE: usize = 0x13200;
pub const SAVE_SIZE: usize = 4096;

pub const SPRITESHEET_BASE: usize = 0x14200;
pub const SPRITESHEET_SIZE: usize = 16384;

pub const TILEMAP_BASE: usize = 0x18200;
pub const TILEMAP_SIZE: usize = 2400;

// Palette is stored in system registers: 256 entries × 2 bytes (RGB565) = 512 bytes
pub const PALETTE_BASE: usize = SYSTEM_BASE;

// Input button bitmask positions
pub const BTN_UP: u16 = 1 << 0;
pub const BTN_DOWN: u16 = 1 << 1;
pub const BTN_LEFT: u16 = 1 << 2;
pub const BTN_RIGHT: u16 = 1 << 3;
pub const BTN_A: u16 = 1 << 4;
pub const BTN_B: u16 = 1 << 5;
pub const BTN_X: u16 = 1 << 6;
pub const BTN_Y: u16 = 1 << 7;
pub const BTN_START: u16 = 1 << 8;
pub const BTN_SELECT: u16 = 1 << 9;
pub const BTN_L: u16 = 1 << 10;
pub const BTN_R: u16 = 1 << 11;

// Input memory layout:
// INPUT_BASE + 0: u16 - current frame buttons
// INPUT_BASE + 2: u16 - previous frame buttons

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Button {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
    A = 4,
    B = 5,
    X = 6,
    Y = 7,
    Start = 8,
    Select = 9,
    L = 10,
    R = 11,
}

// Audio constants
pub const AUDIO_CHANNELS: usize = 4;
// Audio control registers at INPUT_BASE + 4 (after button state)
// Each channel: frequency(u16) + duration(u16) + volume(u8) + waveform(u8) + flags(u8) + padding(u8) = 8 bytes
pub const AUDIO_BASE: usize = INPUT_BASE + 8;
pub const AUDIO_CHANNEL_SIZE: usize = 8;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum Waveform {
    Pulse25 = 0,
    Pulse50 = 1,
    Triangle = 2,
    Noise = 3,
    Sawtooth = 4,
}

// Sprite flags
pub const SPRITE_FLIP_X: u8 = 1 << 0;
pub const SPRITE_FLIP_Y: u8 = 1 << 1;

// Spritesheet dimensions: 128x128 pixels, 8-bit indexed
pub const SPRITESHEET_WIDTH: usize = 128;
pub const SPRITESHEET_HEIGHT: usize = 128;

// Tilemap dimensions
pub const TILEMAP_WIDTH: usize = 40;
pub const TILEMAP_HEIGHT: usize = 30;

// Tilemap scroll registers: stored right after tilemap data
pub const TILEMAP_SCROLL_BASE: usize = TILEMAP_BASE + TILEMAP_SIZE;

// Host-imported functions for operations that need runtime support
unsafe extern "C" {
    fn host_trace(ptr: *const u8, len: u32);
    fn host_random() -> u32;
    fn host_tone(channel: u32, frequency: u32, duration: u32, volume: u32, waveform: u32);
    fn host_quit();
}

/// Request the runtime to exit this game and return to the launcher.
pub fn quit() {
    unsafe { host_quit(); }
}

/// Get a mutable pointer to the base of WASM linear memory.
/// In WASM, address 0 is valid linear memory, but Rust treats null pointer
/// dereference as UB. We use a static byte array at a known location instead,
/// and access memory-mapped regions relative to that.
///
/// We place a sentinel byte array at the start of linear memory and use its
/// address as our base. Since WASM places statics in the data section of
/// linear memory, we instead use raw pointer arithmetic from a known non-null
/// address.
///
/// The approach: store a global at a fixed address and compute base from it.
/// Read a byte from linear memory at the given offset.
#[inline(always)]
pub fn mem_read(offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<u8>(offset)) }
}

/// Write a byte to linear memory at the given offset.
#[inline(always)]
fn mem_write(offset: usize, val: u8) {
    unsafe { core::ptr::write_volatile(core::ptr::without_provenance_mut::<u8>(offset), val) }
}

/// Read a u16 from linear memory at the given offset.
#[inline(always)]
pub fn mem_read_u16(offset: usize) -> u16 {
    unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<u16>(offset)) }
}

/// Write a u16 to linear memory at the given offset.
#[inline(always)]
fn mem_write_u16(offset: usize, val: u16) {
    unsafe { core::ptr::write_volatile(core::ptr::without_provenance_mut::<u16>(offset), val) }
}

/// Read an i32 from linear memory at the given offset.
#[inline(always)]
fn mem_read_i32(offset: usize) -> i32 {
    unsafe { core::ptr::read_volatile(core::ptr::without_provenance::<i32>(offset)) }
}

/// Write an i32 to linear memory at the given offset.
#[inline(always)]
fn mem_write_i32(offset: usize, val: i32) {
    unsafe { core::ptr::write_volatile(core::ptr::without_provenance_mut::<i32>(offset), val) }
}

// === Graphics ===

/// Clear the entire framebuffer to a color index.
pub fn clear(color: u8) {
    for i in 0..FRAMEBUFFER_SIZE {
        mem_write(FRAMEBUFFER_BASE + i, color);
    }
}

/// Set a single pixel. Bounds-checked (no-op if out of range).
pub fn pixel(x: i32, y: i32, color: u8) {
    if x >= 0 && x < SCREEN_WIDTH as i32 && y >= 0 && y < SCREEN_HEIGHT as i32 {
        mem_write(FRAMEBUFFER_BASE + y as usize * SCREEN_WIDTH + x as usize, color);
    }
}

/// Draw a line using Bresenham's algorithm.
pub fn line(x0: i32, y0: i32, x1: i32, y1: i32, color: u8) {
    let mut x0 = x0;
    let mut y0 = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        pixel(x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Draw a rectangle outline.
pub fn rect(x: i32, y: i32, w: u32, h: u32, color: u8) {
    let w = w as i32;
    let h = h as i32;
    line(x, y, x + w - 1, y, color);
    line(x, y + h - 1, x + w - 1, y + h - 1, color);
    line(x, y, x, y + h - 1, color);
    line(x + w - 1, y, x + w - 1, y + h - 1, color);
}

/// Draw a filled rectangle.
pub fn rect_fill(x: i32, y: i32, w: u32, h: u32, color: u8) {
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            pixel(x + dx, y + dy, color);
        }
    }
}

/// Draw a circle outline using the midpoint algorithm.
pub fn circle(cx: i32, cy: i32, r: u32, color: u8) {
    let mut x = r as i32;
    let mut y = 0i32;
    let mut err = 1 - x;

    while x >= y {
        pixel(cx + x, cy + y, color);
        pixel(cx + y, cy + x, color);
        pixel(cx - y, cy + x, color);
        pixel(cx - x, cy + y, color);
        pixel(cx - x, cy - y, color);
        pixel(cx - y, cy - x, color);
        pixel(cx + y, cy - x, color);
        pixel(cx + x, cy - y, color);
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Draw a filled circle.
pub fn circle_fill(cx: i32, cy: i32, r: u32, color: u8) {
    let r = r as i32;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                pixel(cx + dx, cy + dy, color);
            }
        }
    }
}

/// Set a palette entry. Index 0-255, RGB components 0-255.
/// Stored as RGB565 in system registers.
pub fn set_palette(index: u8, r: u8, g: u8, b: u8) {
    let rgb565: u16 = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
    mem_write_u16(PALETTE_BASE + index as usize * 2, rgb565);
}

// Built-in 5x7 bitmap font covering ASCII 32-126
static FONT: [u8; 475] = {
    // Each character is 5 bytes (5 columns × 7 rows packed, but stored as 5 bytes column-major)
    // Characters 32 (space) through 126 (~) = 95 characters × 5 bytes = 475 bytes
    let mut f = [0u8; 475];

    // Space (32)
    // Already zero

    // ! (33)
    f[5] = 0x00; f[6] = 0x00; f[7] = 0x5F; f[8] = 0x00; f[9] = 0x00;
    // " (34)
    f[10] = 0x00; f[11] = 0x07; f[12] = 0x00; f[13] = 0x07; f[14] = 0x00;
    // # (35)
    f[15] = 0x14; f[16] = 0x7F; f[17] = 0x14; f[18] = 0x7F; f[19] = 0x14;
    // $ (36)
    f[20] = 0x24; f[21] = 0x2A; f[22] = 0x7F; f[23] = 0x2A; f[24] = 0x12;
    // % (37)
    f[25] = 0x23; f[26] = 0x13; f[27] = 0x08; f[28] = 0x64; f[29] = 0x62;
    // & (38)
    f[30] = 0x36; f[31] = 0x49; f[32] = 0x55; f[33] = 0x22; f[34] = 0x50;
    // ' (39)
    f[35] = 0x00; f[36] = 0x05; f[37] = 0x03; f[38] = 0x00; f[39] = 0x00;
    // ( (40)
    f[40] = 0x00; f[41] = 0x1C; f[42] = 0x22; f[43] = 0x41; f[44] = 0x00;
    // ) (41)
    f[45] = 0x00; f[46] = 0x41; f[47] = 0x22; f[48] = 0x1C; f[49] = 0x00;
    // * (42)
    f[50] = 0x14; f[51] = 0x08; f[52] = 0x3E; f[53] = 0x08; f[54] = 0x14;
    // + (43)
    f[55] = 0x08; f[56] = 0x08; f[57] = 0x3E; f[58] = 0x08; f[59] = 0x08;
    // , (44)
    f[60] = 0x00; f[61] = 0x50; f[62] = 0x30; f[63] = 0x00; f[64] = 0x00;
    // - (45)
    f[65] = 0x08; f[66] = 0x08; f[67] = 0x08; f[68] = 0x08; f[69] = 0x08;
    // . (46)
    f[70] = 0x00; f[71] = 0x60; f[72] = 0x60; f[73] = 0x00; f[74] = 0x00;
    // / (47)
    f[75] = 0x20; f[76] = 0x10; f[77] = 0x08; f[78] = 0x04; f[79] = 0x02;
    // 0 (48)
    f[80] = 0x3E; f[81] = 0x51; f[82] = 0x49; f[83] = 0x45; f[84] = 0x3E;
    // 1 (49)
    f[85] = 0x00; f[86] = 0x42; f[87] = 0x7F; f[88] = 0x40; f[89] = 0x00;
    // 2 (50)
    f[90] = 0x42; f[91] = 0x61; f[92] = 0x51; f[93] = 0x49; f[94] = 0x46;
    // 3 (51)
    f[95] = 0x21; f[96] = 0x41; f[97] = 0x45; f[98] = 0x4B; f[99] = 0x31;
    // 4 (52)
    f[100] = 0x18; f[101] = 0x14; f[102] = 0x12; f[103] = 0x7F; f[104] = 0x10;
    // 5 (53)
    f[105] = 0x27; f[106] = 0x45; f[107] = 0x45; f[108] = 0x45; f[109] = 0x39;
    // 6 (54)
    f[110] = 0x3C; f[111] = 0x4A; f[112] = 0x49; f[113] = 0x49; f[114] = 0x30;
    // 7 (55)
    f[115] = 0x01; f[116] = 0x71; f[117] = 0x09; f[118] = 0x05; f[119] = 0x03;
    // 8 (56)
    f[120] = 0x36; f[121] = 0x49; f[122] = 0x49; f[123] = 0x49; f[124] = 0x36;
    // 9 (57)
    f[125] = 0x06; f[126] = 0x49; f[127] = 0x49; f[128] = 0x29; f[129] = 0x1E;
    // : (58)
    f[130] = 0x00; f[131] = 0x36; f[132] = 0x36; f[133] = 0x00; f[134] = 0x00;
    // ; (59)
    f[135] = 0x00; f[136] = 0x56; f[137] = 0x36; f[138] = 0x00; f[139] = 0x00;
    // < (60)
    f[140] = 0x08; f[141] = 0x14; f[142] = 0x22; f[143] = 0x41; f[144] = 0x00;
    // = (61)
    f[145] = 0x14; f[146] = 0x14; f[147] = 0x14; f[148] = 0x14; f[149] = 0x14;
    // > (62)
    f[150] = 0x00; f[151] = 0x41; f[152] = 0x22; f[153] = 0x14; f[154] = 0x08;
    // ? (63)
    f[155] = 0x02; f[156] = 0x01; f[157] = 0x51; f[158] = 0x09; f[159] = 0x06;
    // @ (64)
    f[160] = 0x32; f[161] = 0x49; f[162] = 0x79; f[163] = 0x41; f[164] = 0x3E;
    // A (65)
    f[165] = 0x7E; f[166] = 0x11; f[167] = 0x11; f[168] = 0x11; f[169] = 0x7E;
    // B (66)
    f[170] = 0x7F; f[171] = 0x49; f[172] = 0x49; f[173] = 0x49; f[174] = 0x36;
    // C (67)
    f[175] = 0x3E; f[176] = 0x41; f[177] = 0x41; f[178] = 0x41; f[179] = 0x22;
    // D (68)
    f[180] = 0x7F; f[181] = 0x41; f[182] = 0x41; f[183] = 0x22; f[184] = 0x1C;
    // E (69)
    f[185] = 0x7F; f[186] = 0x49; f[187] = 0x49; f[188] = 0x49; f[189] = 0x41;
    // F (70)
    f[190] = 0x7F; f[191] = 0x09; f[192] = 0x09; f[193] = 0x09; f[194] = 0x01;
    // G (71)
    f[195] = 0x3E; f[196] = 0x41; f[197] = 0x49; f[198] = 0x49; f[199] = 0x7A;
    // H (72)
    f[200] = 0x7F; f[201] = 0x08; f[202] = 0x08; f[203] = 0x08; f[204] = 0x7F;
    // I (73)
    f[205] = 0x00; f[206] = 0x41; f[207] = 0x7F; f[208] = 0x41; f[209] = 0x00;
    // J (74)
    f[210] = 0x20; f[211] = 0x40; f[212] = 0x41; f[213] = 0x3F; f[214] = 0x01;
    // K (75)
    f[215] = 0x7F; f[216] = 0x08; f[217] = 0x14; f[218] = 0x22; f[219] = 0x41;
    // L (76)
    f[220] = 0x7F; f[221] = 0x40; f[222] = 0x40; f[223] = 0x40; f[224] = 0x40;
    // M (77)
    f[225] = 0x7F; f[226] = 0x02; f[227] = 0x0C; f[228] = 0x02; f[229] = 0x7F;
    // N (78)
    f[230] = 0x7F; f[231] = 0x04; f[232] = 0x08; f[233] = 0x10; f[234] = 0x7F;
    // O (79)
    f[235] = 0x3E; f[236] = 0x41; f[237] = 0x41; f[238] = 0x41; f[239] = 0x3E;
    // P (80)
    f[240] = 0x7F; f[241] = 0x09; f[242] = 0x09; f[243] = 0x09; f[244] = 0x06;
    // Q (81)
    f[245] = 0x3E; f[246] = 0x41; f[247] = 0x51; f[248] = 0x21; f[249] = 0x5E;
    // R (82)
    f[250] = 0x7F; f[251] = 0x09; f[252] = 0x19; f[253] = 0x29; f[254] = 0x46;
    // S (83)
    f[255] = 0x46; f[256] = 0x49; f[257] = 0x49; f[258] = 0x49; f[259] = 0x31;
    // T (84)
    f[260] = 0x01; f[261] = 0x01; f[262] = 0x7F; f[263] = 0x01; f[264] = 0x01;
    // U (85)
    f[265] = 0x3F; f[266] = 0x40; f[267] = 0x40; f[268] = 0x40; f[269] = 0x3F;
    // V (86)
    f[270] = 0x1F; f[271] = 0x20; f[272] = 0x40; f[273] = 0x20; f[274] = 0x1F;
    // W (87)
    f[275] = 0x3F; f[276] = 0x40; f[277] = 0x38; f[278] = 0x40; f[279] = 0x3F;
    // X (88)
    f[280] = 0x63; f[281] = 0x14; f[282] = 0x08; f[283] = 0x14; f[284] = 0x63;
    // Y (89)
    f[285] = 0x07; f[286] = 0x08; f[287] = 0x70; f[288] = 0x08; f[289] = 0x07;
    // Z (90)
    f[290] = 0x61; f[291] = 0x51; f[292] = 0x49; f[293] = 0x45; f[294] = 0x43;
    // [ (91)
    f[295] = 0x00; f[296] = 0x7F; f[297] = 0x41; f[298] = 0x41; f[299] = 0x00;
    // \ (92)
    f[300] = 0x02; f[301] = 0x04; f[302] = 0x08; f[303] = 0x10; f[304] = 0x20;
    // ] (93)
    f[305] = 0x00; f[306] = 0x41; f[307] = 0x41; f[308] = 0x7F; f[309] = 0x00;
    // ^ (94)
    f[310] = 0x04; f[311] = 0x02; f[312] = 0x01; f[313] = 0x02; f[314] = 0x04;
    // _ (95)
    f[315] = 0x40; f[316] = 0x40; f[317] = 0x40; f[318] = 0x40; f[319] = 0x40;
    // ` (96)
    f[320] = 0x00; f[321] = 0x01; f[322] = 0x02; f[323] = 0x04; f[324] = 0x00;
    // a (97)
    f[325] = 0x20; f[326] = 0x54; f[327] = 0x54; f[328] = 0x54; f[329] = 0x78;
    // b (98)
    f[330] = 0x7F; f[331] = 0x48; f[332] = 0x44; f[333] = 0x44; f[334] = 0x38;
    // c (99)
    f[335] = 0x38; f[336] = 0x44; f[337] = 0x44; f[338] = 0x44; f[339] = 0x20;
    // d (100)
    f[340] = 0x38; f[341] = 0x44; f[342] = 0x44; f[343] = 0x48; f[344] = 0x7F;
    // e (101)
    f[345] = 0x38; f[346] = 0x54; f[347] = 0x54; f[348] = 0x54; f[349] = 0x18;
    // f (102)
    f[350] = 0x08; f[351] = 0x7E; f[352] = 0x09; f[353] = 0x01; f[354] = 0x02;
    // g (103)
    f[355] = 0x0C; f[356] = 0x52; f[357] = 0x52; f[358] = 0x52; f[359] = 0x3E;
    // h (104)
    f[360] = 0x7F; f[361] = 0x08; f[362] = 0x04; f[363] = 0x04; f[364] = 0x78;
    // i (105)
    f[365] = 0x00; f[366] = 0x44; f[367] = 0x7D; f[368] = 0x40; f[369] = 0x00;
    // j (106)
    f[370] = 0x20; f[371] = 0x40; f[372] = 0x44; f[373] = 0x3D; f[374] = 0x00;
    // k (107)
    f[375] = 0x7F; f[376] = 0x10; f[377] = 0x28; f[378] = 0x44; f[379] = 0x00;
    // l (108)
    f[380] = 0x00; f[381] = 0x41; f[382] = 0x7F; f[383] = 0x40; f[384] = 0x00;
    // m (109)
    f[385] = 0x7C; f[386] = 0x04; f[387] = 0x18; f[388] = 0x04; f[389] = 0x78;
    // n (110)
    f[390] = 0x7C; f[391] = 0x08; f[392] = 0x04; f[393] = 0x04; f[394] = 0x78;
    // o (111)
    f[395] = 0x38; f[396] = 0x44; f[397] = 0x44; f[398] = 0x44; f[399] = 0x38;
    // p (112)
    f[400] = 0x7C; f[401] = 0x14; f[402] = 0x14; f[403] = 0x14; f[404] = 0x08;
    // q (113)
    f[405] = 0x08; f[406] = 0x14; f[407] = 0x14; f[408] = 0x18; f[409] = 0x7C;
    // r (114)
    f[410] = 0x7C; f[411] = 0x08; f[412] = 0x04; f[413] = 0x04; f[414] = 0x08;
    // s (115)
    f[415] = 0x48; f[416] = 0x54; f[417] = 0x54; f[418] = 0x54; f[419] = 0x20;
    // t (116)
    f[420] = 0x04; f[421] = 0x3F; f[422] = 0x44; f[423] = 0x40; f[424] = 0x20;
    // u (117)
    f[425] = 0x3C; f[426] = 0x40; f[427] = 0x40; f[428] = 0x20; f[429] = 0x7C;
    // v (118)
    f[430] = 0x1C; f[431] = 0x20; f[432] = 0x40; f[433] = 0x20; f[434] = 0x1C;
    // w (119)
    f[435] = 0x3C; f[436] = 0x40; f[437] = 0x30; f[438] = 0x40; f[439] = 0x3C;
    // x (120)
    f[440] = 0x44; f[441] = 0x28; f[442] = 0x10; f[443] = 0x28; f[444] = 0x44;
    // y (121)
    f[445] = 0x0C; f[446] = 0x50; f[447] = 0x50; f[448] = 0x50; f[449] = 0x3C;
    // z (122)
    f[450] = 0x44; f[451] = 0x64; f[452] = 0x54; f[453] = 0x4C; f[454] = 0x44;
    // { (123)
    f[455] = 0x00; f[456] = 0x08; f[457] = 0x36; f[458] = 0x41; f[459] = 0x00;
    // | (124)
    f[460] = 0x00; f[461] = 0x00; f[462] = 0x7F; f[463] = 0x00; f[464] = 0x00;
    // } (125)
    f[465] = 0x00; f[466] = 0x41; f[467] = 0x36; f[468] = 0x08; f[469] = 0x00;
    // ~ (126)
    f[470] = 0x10; f[471] = 0x08; f[472] = 0x08; f[473] = 0x10; f[474] = 0x10;

    f
};

/// Draw a text string using the built-in 5x7 font.
/// Each character is 6 pixels wide (5 + 1 spacing), 8 pixels tall (7 + 1 spacing).
pub fn text(s: &str, x: i32, y: i32, color: u8) {
    let mut cx = x;
    for byte in s.bytes() {
        if byte >= 32 && byte <= 126 {
            let idx = (byte - 32) as usize * 5;
            for col in 0..5u8 {
                let column_data = FONT[idx + col as usize];
                for row in 0..7u8 {
                    if column_data & (1 << row) != 0 {
                        pixel(cx + col as i32, y + row as i32, color);
                    }
                }
            }
        }
        cx += 6;
    }
}

// === Sprites ===

/// Draw an 8x8 sprite from the spritesheet by tile ID.
/// The spritesheet is 128x128 pixels = 16x16 tiles of 8x8 each (IDs 0-255).
pub fn sprite(id: u16, x: i32, y: i32, flags: u8) {
    let tiles_per_row = (SPRITESHEET_WIDTH / 8) as u16;
    let sx = ((id % tiles_per_row) * 8) as usize;
    let sy = ((id / tiles_per_row) * 8) as usize;
    sprite_region(sx as u32, sy as u32, 8, 8, x, y, flags);
}

/// Draw a rectangular region from the spritesheet.
pub fn sprite_region(sx: u32, sy: u32, sw: u32, sh: u32, dx: i32, dy: i32, flags: u8) {
    let flip_x = flags & SPRITE_FLIP_X != 0;
    let flip_y = flags & SPRITE_FLIP_Y != 0;

    for row in 0..sh {
        for col in 0..sw {
            let src_col = if flip_x { sw - 1 - col } else { col };
            let src_row = if flip_y { sh - 1 - row } else { row };

            let src_offset = (sy + src_row) as usize * SPRITESHEET_WIDTH + (sx + src_col) as usize;
            if src_offset >= SPRITESHEET_SIZE {
                continue;
            }

            let color = mem_read(SPRITESHEET_BASE + src_offset);
            // Color 0 is transparent for sprites
            if color != 0 {
                pixel(dx + col as i32, dy + row as i32, color);
            }
        }
    }
}

// === Tilemap ===

/// Set a tile in the tilemap. tx: 0-39, ty: 0-29, tile_id: sprite tile ID.
pub fn tilemap_set(tx: u32, ty: u32, tile_id: u16) {
    if (tx as usize) < TILEMAP_WIDTH && (ty as usize) < TILEMAP_HEIGHT {
        let offset = TILEMAP_BASE + (ty as usize * TILEMAP_WIDTH + tx as usize) * 2;
        mem_write_u16(offset, tile_id);
    }
}

/// Get a tile from the tilemap.
pub fn tilemap_get(tx: u32, ty: u32) -> u16 {
    if (tx as usize) < TILEMAP_WIDTH && (ty as usize) < TILEMAP_HEIGHT {
        let offset = TILEMAP_BASE + (ty as usize * TILEMAP_WIDTH + tx as usize) * 2;
        mem_read_u16(offset)
    } else {
        0
    }
}

/// Set the tilemap scroll offset.
pub fn tilemap_scroll(dx: i32, dy: i32) {
    mem_write_i32(TILEMAP_SCROLL_BASE, dx);
    mem_write_i32(TILEMAP_SCROLL_BASE + 4, dy);
}

/// Clear the entire tilemap to tile 0.
pub fn tilemap_clear() {
    for i in 0..TILEMAP_SIZE {
        mem_write(TILEMAP_BASE + i, 0);
    }
    mem_write_i32(TILEMAP_SCROLL_BASE, 0);
    mem_write_i32(TILEMAP_SCROLL_BASE + 4, 0);
}

/// Draw the tilemap to the framebuffer. Call this in your draw() function.
pub fn tilemap_draw() {
    let scroll_x = mem_read_i32(TILEMAP_SCROLL_BASE);
    let scroll_y = mem_read_i32(TILEMAP_SCROLL_BASE + 4);

    for ty in 0..TILEMAP_HEIGHT {
        for tx in 0..TILEMAP_WIDTH {
            let offset = TILEMAP_BASE + (ty * TILEMAP_WIDTH + tx) * 2;
            let tile_id = mem_read_u16(offset);
            if tile_id != 0 {
                let px = (tx as i32 * 8) - scroll_x;
                let py = (ty as i32 * 8) - scroll_y;
                sprite(tile_id, px, py, 0);
            }
        }
    }
}

// === Audio ===

/// Play a tone on a channel (0-3).
pub fn tone(channel: u8, frequency: u32, duration: u32, volume: u8, waveform: Waveform) {
    if (channel as usize) < AUDIO_CHANNELS {
        unsafe {
            host_tone(
                channel as u32,
                frequency,
                duration,
                volume as u32,
                waveform as u32,
            );
        }
    }
}

/// Play a frequency sweep on a channel.
pub fn tone_slide(channel: u8, freq_start: u32, freq_end: u32, duration: u32, volume: u8, waveform: Waveform) {
    // Encode start and end frequency: start in lower 16 bits, end in upper 16 bits
    let freq_packed = (freq_start & 0xFFFF) | ((freq_end & 0xFFFF) << 16);
    if (channel as usize) < AUDIO_CHANNELS {
        unsafe {
            host_tone(
                channel as u32 | 0x80, // flag bit 7 = slide mode
                freq_packed,
                duration,
                volume as u32,
                waveform as u32,
            );
        }
    }
}

// === Input ===

fn current_buttons() -> u16 {
    mem_read_u16(INPUT_BASE)
}

fn previous_buttons() -> u16 {
    mem_read_u16(INPUT_BASE + 2)
}

fn button_mask(btn: Button) -> u16 {
    1 << (btn as u16)
}

/// Returns true if the button is currently held down.
pub fn button(btn: Button) -> bool {
    current_buttons() & button_mask(btn) != 0
}

/// Returns true if the button was just pressed this frame.
pub fn button_pressed(btn: Button) -> bool {
    let mask = button_mask(btn);
    (current_buttons() & mask != 0) && (previous_buttons() & mask == 0)
}

/// Returns true if the button was just released this frame.
pub fn button_released(btn: Button) -> bool {
    let mask = button_mask(btn);
    (current_buttons() & mask == 0) && (previous_buttons() & mask != 0)
}

// === System ===

/// Print a debug message to the host console.
pub fn trace(msg: &str) {
    unsafe {
        host_trace(msg.as_ptr(), msg.len() as u32);
    }
}

/// Get a pseudo-random number from the host.
pub fn random() -> u32 {
    unsafe { host_random() }
}

static mut FRAME_COUNTER: u32 = 0;

/// Get the number of frames elapsed since boot.
pub fn frame_count() -> u32 {
    unsafe { FRAME_COUNTER }
}

/// Called by the runtime each frame to increment the counter.
/// Not meant to be called by games directly.
#[doc(hidden)]
#[unsafe(no_mangle)]
pub extern "C" fn _sdk_tick() {
    unsafe {
        FRAME_COUNTER += 1;
    }
}

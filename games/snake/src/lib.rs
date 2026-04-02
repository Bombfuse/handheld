#![no_std]

use handheld_sdk::*;

const GRID_W: i32 = 40;
const GRID_H: i32 = 28;
const GRID_Y_OFFSET: i32 = 2;
const CELL: i32 = 8;
const MAX_SNAKE: usize = 512;
const MOVE_INTERVAL: u32 = 8;

const COL_BG: u8 = 0;
const COL_TEXT: u8 = 1;
const COL_SNAKE_HEAD: u8 = 2;
const COL_SNAKE_BODY: u8 = 10;
const COL_FOOD: u8 = 4;
const COL_BORDER: u8 = 9;
const COL_GAMEOVER: u8 = 5;

// Local PRNG to avoid importing host_random (which triggers a k23 bug with function tables)
static mut RNG_STATE: u32 = 12345;

fn local_random() -> u32 {
    unsafe {
        RNG_STATE ^= RNG_STATE << 13;
        RNG_STATE ^= RNG_STATE >> 17;
        RNG_STATE ^= RNG_STATE << 5;
        RNG_STATE
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Pos { x: i32, y: i32 }

#[derive(Clone, Copy, PartialEq)]
enum Dir { Up, Down, Left, Right }

impl Dir {
    fn dx(self) -> i32 { match self { Dir::Left => -1, Dir::Right => 1, _ => 0 } }
    fn dy(self) -> i32 { match self { Dir::Up => -1, Dir::Down => 1, _ => 0 } }
}

#[derive(Clone, Copy, PartialEq)]
enum State { Title, Playing, GameOver }

static mut SNAKE: [Pos; MAX_SNAKE] = [Pos { x: 0, y: 0 }; MAX_SNAKE];
static mut LEN: usize = 0;
static mut DIR: Dir = Dir::Right;
static mut NEXT_DIR: Dir = Dir::Right;
static mut FOOD: Pos = Pos { x: 0, y: 0 };
static mut SCORE: u32 = 0;
static mut HIGH_SCORE: u32 = 0;
static mut GAME_STATE: State = State::Title;
static mut MOVE_TIMER: u32 = 0;

fn snake() -> &'static [Pos] { unsafe { &SNAKE[..LEN] } }

fn reset() {
    unsafe {
        LEN = 3;
        DIR = Dir::Right;
        NEXT_DIR = Dir::Right;
        SCORE = 0;
        MOVE_TIMER = 0;
        let sx = GRID_W / 2;
        let sy = GRID_H / 2;
        for i in 0..LEN {
            SNAKE[i] = Pos { x: sx - i as i32, y: sy };
        }
        // Seed RNG with frame count for variety
        RNG_STATE = frame_count().wrapping_add(12345);
    }
    spawn_food();
}

fn spawn_food() {
    loop {
        let x = (local_random() % GRID_W as u32) as i32;
        let y = (local_random() % GRID_H as u32) as i32;
        let pos = Pos { x, y };
        let mut on_snake = false;
        let len = unsafe { LEN };
        for i in 0..len {
            if unsafe { SNAKE[i] } == pos { on_snake = true; break; }
        }
        if !on_snake {
            unsafe { FOOD = pos; }
            break;
        }
    }
}

fn game_update() {
    unsafe {
        match GAME_STATE {
            State::Title => {
                if button_pressed(Button::Start) || button_pressed(Button::A) {
                    GAME_STATE = State::Playing;
                    reset();
                }
            }
            State::Playing => {
                if button_pressed(Button::Up) && DIR != Dir::Down { NEXT_DIR = Dir::Up; }
                else if button_pressed(Button::Down) && DIR != Dir::Up { NEXT_DIR = Dir::Down; }
                else if button_pressed(Button::Left) && DIR != Dir::Right { NEXT_DIR = Dir::Left; }
                else if button_pressed(Button::Right) && DIR != Dir::Left { NEXT_DIR = Dir::Right; }

                MOVE_TIMER += 1;
                if MOVE_TIMER >= MOVE_INTERVAL {
                    MOVE_TIMER = 0;
                    DIR = NEXT_DIR;
                    step();
                }
            }
            State::GameOver => {
                if button_pressed(Button::Start) || button_pressed(Button::A) {
                    GAME_STATE = State::Playing;
                    reset();
                }
            }
        }
    }
}

fn step() {
    unsafe {
        let head = SNAKE[0];
        let new_head = Pos { x: head.x + DIR.dx(), y: head.y + DIR.dy() };

        if new_head.x < 0 || new_head.x >= GRID_W || new_head.y < 0 || new_head.y >= GRID_H {
            die(); return;
        }
        for i in 0..LEN {
            if SNAKE[i] == new_head { die(); return; }
        }

        let ate = new_head == FOOD;
        if ate {
            if LEN < MAX_SNAKE {
                let mut i = LEN;
                while i > 0 { SNAKE[i] = SNAKE[i - 1]; i -= 1; }
                LEN += 1;
            }
        } else {
            let mut i = LEN - 1;
            while i > 0 { SNAKE[i] = SNAKE[i - 1]; i -= 1; }
        }
        SNAKE[0] = new_head;

        if ate {
            SCORE += 10;
            if SCORE > HIGH_SCORE { HIGH_SCORE = SCORE; }
            spawn_food();
        }
    }
}

fn die() {
    unsafe {
        GAME_STATE = State::GameOver;
        if SCORE > HIGH_SCORE { HIGH_SCORE = SCORE; }
    }
}

fn game_draw() {
    clear(COL_BG);
    unsafe {
        match GAME_STATE {
            State::Title => draw_title(),
            State::Playing => draw_game(),
            State::GameOver => { draw_game(); draw_game_over(); }
        }
    }
}

fn draw_title() {
    text("SNAKE", 130, 60, COL_TEXT);
    text("for Handheld OS", 88, 80, COL_SNAKE_BODY);

    let f = frame_count();
    let wave = ((f / 10) % 6) as i32;
    for i in 0..8i32 {
        let y = 130 + if (i + wave) % 3 == 0 { -2 } else { 0 };
        rect_fill(120 + i * 10, y, 8, 8, if i == 7 { COL_SNAKE_HEAD } else { COL_SNAKE_BODY });
    }
    if (f / 20) % 2 == 0 { rect_fill(220, 128, 8, 8, COL_FOOD); }
    text("Press START", 112, 180, COL_TEXT);

    unsafe {
        if HIGH_SCORE > 0 {
            text("HI:", 128, 200, COL_TEXT);
            draw_number(HIGH_SCORE, 152, 200, COL_GAMEOVER);
        }
    }
}

fn draw_game() {
    unsafe {
        text("SCORE:", 4, 4, COL_TEXT);
        draw_number(SCORE, 44, 4, COL_TEXT);
        text("HI:", 240, 4, COL_TEXT);
        draw_number(HIGH_SCORE, 264, 4, COL_TEXT);

        let play_y = GRID_Y_OFFSET * CELL;
        rect(0, play_y - 1, (GRID_W * CELL) as u32, (GRID_H * CELL + 2) as u32, COL_BORDER);

        let blink = (frame_count() / 8) % 2 == 0;
        let food_color = if blink { COL_FOOD } else { COL_GAMEOVER };
        rect_fill(FOOD.x * CELL + 1, (FOOD.y + GRID_Y_OFFSET) * CELL + 1, 6, 6, food_color);

        for i in 0..LEN {
            let p = SNAKE[i];
            let color = if i == 0 { COL_SNAKE_HEAD } else { COL_SNAKE_BODY };
            let px = p.x * CELL;
            let py = (p.y + GRID_Y_OFFSET) * CELL;
            if i == 0 {
                rect_fill(px, py, CELL as u32, CELL as u32, color);
            } else {
                rect_fill(px + 1, py + 1, (CELL - 2) as u32, (CELL - 2) as u32, color);
            }
        }
    }
}

fn draw_game_over() {
    let mut y = 60i32;
    while y < 140 {
        let mut x = 0i32;
        while x < 320 {
            pixel(x, y, COL_BG);
            x += 2;
        }
        y += 2;
    }
    rect_fill(60, 80, 200, 48, COL_BG);
    rect(60, 80, 200, 48, COL_GAMEOVER);
    text("GAME OVER", 112, 90, COL_GAMEOVER);
    text("Score:", 100, 108, COL_TEXT);
    unsafe { draw_number(SCORE, 144, 108, COL_TEXT); }
    text("Press START", 104, 140, COL_TEXT);
}

fn digit_str(d: u32) -> &'static str {
    match d {
        0 => "0", 1 => "1", 2 => "2", 3 => "3", 4 => "4",
        5 => "5", 6 => "6", 7 => "7", 8 => "8", _ => "9",
    }
}

fn draw_number(mut n: u32, x: i32, y: i32, color: u8) {
    if n == 0 { text("0", x, y, color); return; }
    let mut buf = [0u32; 10];
    let mut len = 0usize;
    while n > 0 && len < 10 {
        buf[len] = n % 10;
        n /= 10;
        len += 1;
    }
    let mut cx = x;
    let mut i = len;
    while i > 0 {
        i -= 1;
        text(digit_str(buf[i]), cx, y, color);
        cx += 6;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    set_palette(0, 16, 16, 24);
    set_palette(1, 220, 220, 220);
    set_palette(2, 0, 220, 80);
    set_palette(3, 80, 120, 255);
    set_palette(4, 255, 60, 60);
    set_palette(5, 255, 200, 0);
    set_palette(9, 60, 60, 80);
    set_palette(10, 0, 180, 60);
}

#[unsafe(no_mangle)]
pub extern "C" fn update() { game_update(); }

#[unsafe(no_mangle)]
pub extern "C" fn draw() { game_draw(); }

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

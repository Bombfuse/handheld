// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Basic kernel shell for debugging purposes, taken from
//! <https://github.com/hawkw/mycelium/blob/main/src/shell.rs> (MIT)

const S: &str = r#"
   __    ___  ____
  / /__ |_  ||_  /
 /  '_// __/_/_ <
/_/\_\/____/____/
"#;

use alloc::string::{String, ToString};
use core::fmt;
use core::fmt::Write;
use core::ops::{DerefMut, Range};
use core::str::FromStr;

use fallible_iterator::FallibleIterator;
use kasync::executor::Executor;
use kmem_core::{AddressRangeExt, PhysicalAddress};
use kspin::{Barrier, OnceLock};

use crate::device_tree::DeviceTree;
use crate::mem::{Mmap, with_kernel_aspace};
use crate::state::global;
use crate::{arch, irq};

static COMMANDS: &[Command] = &[RUN, PANIC, FAULT, VERSION, SHUTDOWN];

pub fn init(devtree: &'static DeviceTree, sched: &'static Executor, num_cpus: usize) {
    // The `Barrier` below is here so that the maybe verbose startup logging is
    // out of the way before dropping the user into the kernel shell. If we don't
    // wait for the last CPU to have finished initializing it will mess up the shell output.
    static SYNC: OnceLock<Barrier> = OnceLock::new();
    let barrier = SYNC.get_or_init(|| Barrier::new(num_cpus));

    if barrier.wait().is_leader() {
        tracing::info!("{S}");

        // Auto-run the embedded game on boot with ramfb display
        tracing::info!("Starting game with ramfb display...");
        if let Err(e) = run_game_visual() {
            tracing::error!("Game failed: {e}");
        }

        tracing::info!("type `help` to list available commands");

        sched
            .try_spawn(async move {
                let (mut uart, _mmap, irq_num) = init_uart(devtree);

                let mut line = String::new();
                loop {
                    let res = irq::next_event(irq_num).await;
                    assert!(res.is_ok());
                    let mut newline = false;

                    let ch = uart.recv() as char;
                    uart.write_char(ch).unwrap();
                    match ch {
                        '\n' | '\r' => {
                            newline = true;
                            uart.write_str("\n\r").unwrap();
                        }
                        '\u{007F}' => {
                            line.pop();
                        }
                        ch => line.push(ch),
                    }

                    if newline {
                        eval(&line);
                        line.clear();
                    }
                }
            })
            .unwrap();
    }
}

fn init_uart(devtree: &DeviceTree) -> (kuart_16550::SerialPort, Mmap, u32) {
    let s = devtree.find_by_path("/soc/serial").unwrap();
    assert!(s.is_compatible(["ns16550a"]));

    let clock_freq = s.property("clock-frequency").unwrap().as_u32().unwrap();
    let mut regs = s.regs().unwrap();
    let reg = regs.next().unwrap().unwrap();
    assert!(regs.next().unwrap().is_none());
    let irq_num = s.property("interrupts").unwrap().as_u32().unwrap();

    let mmap = with_kernel_aspace(|aspace| {
        // FIXME: this is gross, we're using the PhysicalAddress as an alignment utility :/
        let size = PhysicalAddress::new(reg.size.unwrap())
            .align_up(arch::PAGE_SIZE)
            .get();

        let range_phys = Range::from_start_len(PhysicalAddress::new(reg.starting_address), size);

        let mmap = Mmap::new_phys(
            aspace.clone(),
            range_phys,
            size,
            arch::PAGE_SIZE,
            Some("UART-16550".to_string()),
        )
        .unwrap();

        mmap.commit(aspace.lock().deref_mut(), 0..size, true)
            .unwrap();

        mmap
    });

    // Safety: info comes from device tree
    let uart =
        unsafe { kuart_16550::SerialPort::new(mmap.range().start.get(), clock_freq, 115200) };

    (uart, mmap, irq_num)
}

pub fn eval(line: &str) {
    if line == "help" {
        tracing::info!(target: "shell", "available commands:");
        print_help("", COMMANDS);
        tracing::info!(target: "shell", "");
        return;
    }

    match handle_command(Context::new(line), COMMANDS) {
        Ok(_) => {}
        Err(error) => tracing::error!(target: "shell", "error: {error}"),
    }
}

const RUN: Command = Command::new("run")
    .with_help("run the embedded WASM game module")
    .with_fn(|_| {
        // Can't easily get device tree here, so just print info
        tracing::info!("Game auto-runs on boot. Use `shutdown` and reboot to restart.");
        Ok(())
    });

fn run_game_visual() -> crate::Result<()> {
    use alloc::string::String;
    use alloc::vec::Vec;
    use wasmparser::Validator;
    use crate::ramfb::Ramfb;
    use crate::wasm::{
        ConstExprEvaluator, Engine, Func, Linker, Memory, Module, PlaceholderAllocatorDontUse, Store,
    };

    // Embedded cart files and launcher
    static LAUNCHER_WASM: &[u8] = include_bytes!("launcher.wasm");
    static CART_HELLO: &[u8] = include_bytes!("hello.cart");
    static CART_SNAKE: &[u8] = include_bytes!("snake.cart");

    static CARTS: &[&[u8]] = &[CART_HELLO, CART_SNAKE];

    // Parse cart names
    let mut cart_names: Vec<String> = Vec::new();
    for cart_data in CARTS {
        let name = if let Ok(reader) = handheld_cart::CartReader::new(cart_data) {
            reader.meta().map(|m| String::from(m.name())).unwrap_or_else(|| String::from("Unknown"))
        } else {
            String::from("Unknown")
        };
        cart_names.push(name);
    }
    tracing::info!("Loaded {} carts: {:?}", cart_names.len(), cart_names);

    // Shared state for host functions
    use core::sync::atomic::{AtomicI32, AtomicU32, Ordering as AO};
    static PENDING_LAUNCH: AtomicI32 = AtomicI32::new(-1);  // -1 = none, -2 = quit, >=0 = game index
    static GAME_COUNT: AtomicU32 = AtomicU32::new(0);
    // Store names in a static buffer for host_game_name
    static mut NAME_BUF: [[u8; 32]; 16] = [[0u8; 32]; 16];
    static mut NAME_LENS: [u8; 16] = [0; 16];

    GAME_COUNT.store(cart_names.len() as u32, AO::Relaxed);
    for (i, name) in cart_names.iter().enumerate() {
        if i >= 16 { break; }
        let bytes = name.as_bytes();
        let len = bytes.len().min(32);
        unsafe {
            NAME_BUF[i][..len].copy_from_slice(&bytes[..len]);
            NAME_LENS[i] = len as u8;
        }
    }

    // Initialize ramfb display
    tracing::info!("Initializing ramfb...");
    let mut ramfb = Ramfb::init()?;

    // Initialize VirtIO keyboard
    let mut kbd = crate::virtio_input::VirtioInput::init()?;
    tracing::info!("Controls: click QEMU window, arrows/wasd, Enter=Start, q=quit");

    let engine = Engine::default();

    // Helper to create a linker with all host functions
    let create_linker = |engine: &Engine| -> crate::Result<Linker<()>> {
        let mut linker = Linker::<()>::new(engine);
        linker.func_wrap("env", "host_trace", |_: u32, _: u32| {})?;
        linker.func_wrap("env", "host_random", || -> u32 {
            static mut SEED: u32 = 12345;
            unsafe { SEED ^= SEED << 13; SEED ^= SEED >> 17; SEED ^= SEED << 5; SEED }
        })?;
        linker.func_wrap("env", "host_tone", |_: u32, _: u32, _: u32, _: u32, _: u32| {})?;
        linker.func_wrap("env", "host_quit", || {
            PENDING_LAUNCH.store(-2, AO::Relaxed);
        })?;
        linker.func_wrap("env", "host_game_count", || -> u32 {
            GAME_COUNT.load(AO::Relaxed)
        })?;
        linker.func_wrap("env", "host_game_name", |index: u32, ptr: u32, len: u32| -> u32 {
            // Can't access WASM memory from here without Caller, so return 0
            // The launcher will need to use a different approach on k23
            0u32
        })?;
        linker.func_wrap("env", "host_launch_game", |index: u32| {
            PENDING_LAUNCH.store(index as i32, AO::Relaxed);
        })?;
        Ok(linker)
    };

    // Helper to load and init a WASM module
    struct RunningModule {
        store: Store<()>,
        init_fn: Option<Func>,
        update_fn: Option<Func>,
        draw_fn: Option<Func>,
        tick_fn: Option<Func>,
        memory: Memory,
    }

    let load_module = |engine: &Engine, linker: &Linker<()>, wasm: &[u8]| -> crate::Result<RunningModule> {
        let mut validator = Validator::new();
        tracing::info!("Compiling {} bytes...", wasm.len());
        let module = Module::from_bytes(engine, &mut validator, wasm)?;
        tracing::info!("Compiled. Instantiating...");
        let mut store = Store::new(engine, &PlaceholderAllocatorDontUse, ());
        let mut const_eval = ConstExprEvaluator::default();
        let instance = linker.instantiate(&mut store, &mut const_eval, &module)?;
        tracing::info!("Instantiated.");
        let memory = instance.get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow::anyhow!("no memory export"))?;
        let update_fn = instance.get_func(&mut store, "update");
        let draw_fn = instance.get_func(&mut store, "draw");
        let tick_fn = instance.get_func(&mut store, "_sdk_tick");
        let init_fn = instance.get_func(&mut store, "init");
        Ok(RunningModule { store, update_fn, draw_fn, tick_fn, init_fn, memory })
    };

    let linker = create_linker(&engine)?;

    let write_names = |m: &mut RunningModule, names: &Vec<String>| {
        let data = m.memory.data_mut(&mut m.store);
        let name_base: usize = 0x200 + 16;
        let count = names.len().min(16) as u32;
        data[name_base..name_base + 4].copy_from_slice(&count.to_le_bytes());
        for (i, name) in names.iter().enumerate() {
            if i >= 16 { break; }
            let offset = name_base + 4 + i * 32;
            let bytes = name.as_bytes();
            let len = bytes.len().min(31);
            data[offset] = len as u8;
            data[offset + 1..offset + 1 + len].copy_from_slice(&bytes[..len]);
        }
    };

    let call_init = |m: &mut RunningModule| -> crate::Result<()> {
        if let Some(f) = m.init_fn { f.call(&mut m.store, &[], &mut [])?; }
        Ok(())
    };

    // Start with launcher — write names BEFORE init
    tracing::info!("Loading launcher...");
    let mut current = load_module(&engine, &linker, LAUNCHER_WASM)?;
    write_names(&mut current, &cart_names);
    call_init(&mut current)?;
    tracing::info!("Launcher ready");

    let mut palette = [(0u8, 0u8, 0u8); 256];
    const FB_OFFSET: usize = 0x300;
    const PALETTE_OFFSET: usize = 0x0;
    const INPUT_OFFSET: usize = 0x200;
    const WIDTH: usize = 320;
    const HEIGHT: usize = 240;

    PENDING_LAUNCH.store(-1, AO::Relaxed);

    loop {
        kbd.poll_events();
        let buttons = kbd.buttons();
        if kbd.quit_requested() {
            // Q pressed — return to launcher instead of exiting
            PENDING_LAUNCH.store(-2, AO::Relaxed);
            kbd.clear_quit();
        }

        // Write input
        {
            let data = current.memory.data_mut(&mut current.store);
            let prev = u16::from_le_bytes([data[INPUT_OFFSET], data[INPUT_OFFSET + 1]]);
            data[INPUT_OFFSET] = (buttons & 0xFF) as u8;
            data[INPUT_OFFSET + 1] = (buttons >> 8) as u8;
            data[INPUT_OFFSET + 2] = (prev & 0xFF) as u8;
            data[INPUT_OFFSET + 3] = (prev >> 8) as u8;
        }

        // Tick + update + draw
        if let Some(f) = current.tick_fn { f.call(&mut current.store, &[], &mut [])?; }
        if let Some(f) = current.update_fn { f.call(&mut current.store, &[], &mut [])?; }
        if let Some(f) = current.draw_fn { f.call(&mut current.store, &[], &mut [])?; }

        // Check pending action
        let pending = PENDING_LAUNCH.load(AO::Relaxed);
        if pending >= 0 {
            let idx = pending as usize;
            PENDING_LAUNCH.store(-1, AO::Relaxed);
            if idx < CARTS.len() {
                let cart_data = CARTS[idx];
                if let Ok(reader) = handheld_cart::CartReader::new(cart_data) {
                    if let Some(wasm) = reader.wasm() {
                        tracing::info!("Launching game {} ({} bytes WASM)...", idx, wasm.len());
                        // Keep old module alive (don't drop) to test if the issue is in drop
                        core::mem::forget(current);
                        match load_module(&engine, &linker, wasm) {
                            Ok(mut m) => {
                                tracing::info!("Module loaded, calling init...");
                                if let Err(e) = call_init(&mut m) {
                                    tracing::error!("init() failed: {e}");
                                }
                                current = m;
                                tracing::info!("Game running");
                            }
                            Err(e) => {
                                tracing::error!("Failed to load game: {e}");
                                // Reload launcher as fallback
                                current = load_module(&engine, &linker, LAUNCHER_WASM)?;
                                write_names(&mut current, &cart_names);
                                call_init(&mut current)?;
                            }
                        }
                        continue;
                    }
                }
            }
        } else if pending == -2 {
            // Return to launcher
            PENDING_LAUNCH.store(-1, AO::Relaxed);
            tracing::info!("Returning to launcher...");
            drop(current);
            current = load_module(&engine, &linker, LAUNCHER_WASM)?;
            write_names(&mut current, &cart_names);
            call_init(&mut current)?;
            continue;
        }

        // Read palette and blit
        {
            let data = current.memory.data(&current.store);
            for i in 0..256 {
                let off = PALETTE_OFFSET + i * 2;
                let rgb565 = u16::from_le_bytes([data[off], data[off + 1]]);
                let r = ((rgb565 >> 11) & 0x1F) as u8;
                let g = ((rgb565 >> 5) & 0x3F) as u8;
                let b = (rgb565 & 0x1F) as u8;
                palette[i] = ((r as u32 * 255 / 31) as u8, (g as u32 * 255 / 63) as u8, (b as u32 * 255 / 31) as u8);
            }
        }
        let fb_data = &current.memory.data(&current.store)[FB_OFFSET..FB_OFFSET + WIDTH * HEIGHT];
        ramfb.blit(fb_data, &palette);

        for _ in 0..160_000 { core::hint::spin_loop(); }
    }

    tracing::info!("Game loop finished");
    Ok(())
}

const PANIC: Command = Command::new("panic")
    .with_usage("<MESSAGE>")
    .with_help("cause a kernel panic with the given message. use with caution.")
    .with_fn(|line| {
        panic!("{}", line.current);
    });

const FAULT: Command = Command::new("fault")
    .with_help("cause a CPU fault (null pointer dereference). use with caution.")
    .with_fn(|_| {
        // Safety: This actually *is* unsafe and *is* causing problematic behaviour, but that is exactly what
        // we want here!
        unsafe {
            core::ptr::dangling::<u8>().read_volatile();
        }
        Ok(())
    });

const VERSION: Command = Command::new("version")
    .with_help("print verbose build and version info.")
    .with_fn(|_| {
        tracing::info!("k23 v{}", env!("CARGO_PKG_VERSION"));
        // TODO reimplement this with vergen later
        // tracing::info!(build.version = %concat!(
        //     env!("CARGO_PKG_VERSION"),
        //     "-",
        //     env!("VERGEN_GIT_BRANCH"),
        //     ".",
        //     env!("VERGEN_GIT_SHA")
        // ));
        // tracing::info!(build.timestamp = %env!("VERGEN_BUILD_TIMESTAMP"));
        // tracing::info!(build.opt_level = %env!("VERGEN_CARGO_OPT_LEVEL"));
        // tracing::info!(build.target = %env!("VERGEN_CARGO_TARGET_TRIPLE"));
        // tracing::info!(commit.sha = %env!("VERGEN_GIT_SHA"));
        // tracing::info!(commit.branch = %env!("VERGEN_GIT_BRANCH"));
        // tracing::info!(commit.date = %env!("VERGEN_GIT_COMMIT_TIMESTAMP"));
        // tracing::info!(rustc.version = %env!("VERGEN_RUSTC_SEMVER"));
        // tracing::info!(rustc.channel = %env!("VERGEN_RUSTC_CHANNEL"));

        Ok(())
    });

const SHUTDOWN: Command = Command::new("shutdown")
    .with_help("exit the kernel and shutdown the machine.")
    .with_fn(|_| {
        tracing::info!("Bye, Bye!");

        global().executor.close();

        Ok(())
    });

#[derive(Debug)]
pub struct Command<'cmd> {
    name: &'cmd str,
    help: &'cmd str,
    usage: &'cmd str,
    run: fn(Context<'_>) -> CmdResult<'_>,
}

pub type CmdResult<'a> = Result<(), Error<'a>>;

#[derive(Debug)]
pub struct Error<'a> {
    line: &'a str,
    kind: ErrorKind<'a>,
}

#[derive(Debug)]
enum ErrorKind<'a> {
    UnknownCommand(&'a [Command<'a>]),
    InvalidArguments {
        help: &'a str,
        arg: &'a str,
        flag: Option<&'a str>,
    },
    FlagRequired {
        flags: &'a [&'a str],
    },
    Other(&'static str),
}

#[derive(Copy, Clone)]
pub struct Context<'cmd> {
    line: &'cmd str,
    current: &'cmd str,
}

fn print_help(parent_cmd: &str, commands: &[Command]) {
    let parent_cmd_pad = if parent_cmd.is_empty() { "" } else { " " };
    for command in commands {
        tracing::info!(target: "shell", "  {parent_cmd}{parent_cmd_pad}{command}");
    }
    tracing::info!(target: "shell", "  {parent_cmd}{parent_cmd_pad}help --- prints this help message");
}

fn handle_command<'cmd>(ctx: Context<'cmd>, commands: &'cmd [Command]) -> CmdResult<'cmd> {
    let chunk = ctx.current.trim();
    for cmd in commands {
        if let Some(current) = chunk.strip_prefix(cmd.name) {
            let current = current.trim();

            return kpanic_unwind::catch_unwind(|| cmd.run(Context { current, ..ctx })).unwrap_or(
                {
                    Err(Error {
                        line: cmd.name,
                        kind: ErrorKind::Other("command failed"),
                    })
                },
            );
        }
    }

    Err(ctx.unknown_command(commands))
}

// === impl Command ===

impl<'cmd> Command<'cmd> {
    #[must_use]
    pub const fn new(name: &'cmd str) -> Self {
        #[cold]
        fn invalid_command(_ctx: Context<'_>) -> CmdResult<'_> {
            panic!("command is missing run function, this is a bug");
        }

        Self {
            name,
            help: "",
            usage: "",
            run: invalid_command,
        }
    }

    #[must_use]
    pub const fn with_help(self, help: &'cmd str) -> Self {
        Self { help, ..self }
    }

    #[must_use]
    pub const fn with_usage(self, usage: &'cmd str) -> Self {
        Self { usage, ..self }
    }

    #[must_use]
    pub const fn with_fn(self, run: fn(Context<'_>) -> CmdResult<'_>) -> Self {
        Self { run, ..self }
    }

    pub fn run<'ctx>(&'cmd self, ctx: Context<'ctx>) -> CmdResult<'ctx>
    where
        'cmd: 'ctx,
    {
        let current = ctx.current.trim();

        if current == "help" {
            let name = ctx.line.strip_suffix(" help").unwrap_or("<???BUG???>");
            tracing::info!(target: "shell", "{name}");

            return Ok(());
        }

        (self.run)(ctx)
    }
}

impl fmt::Display for Command<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            run: _func,
            name,
            help,
            usage,
        } = self;

        write!(
            f,
            "{name}{usage_pad}{usage} --- {help}",
            usage_pad = if !usage.is_empty() { " " } else { "" },
        )
    }
}

// === impl Error ===

impl fmt::Display for Error<'_> {
    fn fmt(&self, mut f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn command_names<'cmd>(
            cmds: &'cmd [Command<'cmd>],
        ) -> impl Iterator<Item = &'cmd str> + 'cmd {
            cmds.iter()
                .map(|Command { name, .. }| *name)
                .chain(core::iter::once("help"))
        }

        fn fmt_flag_names(f: &mut fmt::Formatter<'_>, flags: &[&str]) -> fmt::Result {
            let mut names = flags.iter();
            if let Some(name) = names.next() {
                f.write_str(name)?;
                for name in names {
                    write!(f, "|{name}")?;
                }
            }
            Ok(())
        }

        let Self { line, kind } = self;
        match kind {
            ErrorKind::UnknownCommand(commands) => {
                write!(f, "unknown command {line:?}, expected one of: [")?;
                comma_delimited(&mut f, command_names(commands))?;
                f.write_char(']')?;
            }
            ErrorKind::InvalidArguments { help, arg, flag } => {
                f.write_str("invalid argument")?;
                if let Some(flag) = flag {
                    write!(f, " {flag}")?;
                }
                write!(f, " {arg:?}: {help}")?;
            }
            ErrorKind::FlagRequired { flags } => {
                write!(f, "the '{line}' command requires the ")?;
                fmt_flag_names(f, flags)?;
                write!(f, " flag")?;
            }
            ErrorKind::Other(msg) => write!(f, "could not execute {line:?}: {msg}")?,
        }

        Ok(())
    }
}

impl core::error::Error for Error<'_> {}

fn comma_delimited<F: fmt::Display>(
    mut writer: impl Write,
    values: impl IntoIterator<Item = F>,
) -> fmt::Result {
    let mut values = values.into_iter();
    if let Some(value) = values.next() {
        write!(writer, "{value}")?;
        for value in values {
            write!(writer, ", {value}")?;
        }
    }

    Ok(())
}

// === impl Context ===

impl<'cmd> Context<'cmd> {
    pub const fn new(line: &'cmd str) -> Self {
        Self {
            line,
            current: line,
        }
    }

    pub fn command(&self) -> &'cmd str {
        self.current.trim()
    }

    fn unknown_command(&self, commands: &'cmd [Command]) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::UnknownCommand(commands),
        }
    }

    pub fn invalid_argument(&self, help: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::InvalidArguments {
                arg: self.current,
                flag: None,
                help,
            },
        }
    }

    pub fn invalid_argument_named(&self, name: &'static str, help: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::InvalidArguments {
                arg: self.current,
                flag: Some(name),
                help,
            },
        }
    }

    pub fn other_error(&self, msg: &'static str) -> Error<'cmd> {
        Error {
            line: self.line,
            kind: ErrorKind::Other(msg),
        }
    }

    pub fn parse_bool_flag(&mut self, flag: &str) -> bool {
        if let Some(rest) = self.command().trim().strip_prefix(flag) {
            self.current = rest.trim();
            true
        } else {
            false
        }
    }

    pub fn parse_optional_u32_hex_or_dec(
        &mut self,
        name: &'static str,
    ) -> Result<Option<u32>, Error<'cmd>> {
        let (chunk, rest) = match self.command().split_once(" ") {
            Some((chunk, rest)) => (chunk.trim(), rest),
            None => (self.command(), ""),
        };

        if chunk.is_empty() {
            return Ok(None);
        }

        let val = if let Some(hex_num) = chunk.strip_prefix("0x") {
            u32::from_str_radix(hex_num.trim(), 16).map_err(|_| Error {
                line: self.line,
                kind: ErrorKind::InvalidArguments {
                    arg: chunk,
                    flag: Some(name),
                    help: "expected a 32-bit hex number",
                },
            })?
        } else {
            u32::from_str(chunk).map_err(|_| Error {
                line: self.line,
                kind: ErrorKind::InvalidArguments {
                    arg: chunk,
                    flag: Some(name),
                    help: "expected a 32-bit decimal number",
                },
            })?
        };

        self.current = rest;
        Ok(Some(val))
    }

    pub fn parse_u32_hex_or_dec(&mut self, name: &'static str) -> Result<u32, Error<'cmd>> {
        self.parse_optional_u32_hex_or_dec(name).and_then(|val| {
            val.ok_or_else(|| self.invalid_argument_named(name, "expected a number"))
        })
    }

    pub fn parse_optional_flag<T>(
        &mut self,
        names: &'static [&'static str],
    ) -> Result<Option<T>, Error<'cmd>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        for name in names {
            if let Some(rest) = self.command().strip_prefix(name) {
                let (chunk, rest) = match rest.trim().split_once(" ") {
                    Some((chunk, rest)) => (chunk.trim(), rest),
                    None => (rest, ""),
                };

                if chunk.is_empty() {
                    return Err(Error {
                        line: self.line,
                        kind: ErrorKind::InvalidArguments {
                            arg: chunk,
                            flag: Some(name),
                            help: "expected a value",
                        },
                    });
                }

                match chunk.parse() {
                    Ok(val) => {
                        self.current = rest;
                        return Ok(Some(val));
                    }
                    Err(e) => {
                        tracing::warn!(target: "shell", "invalid value {chunk:?} for flag {name}: {e}");
                        return Err(Error {
                            line: self.line,
                            kind: ErrorKind::InvalidArguments {
                                arg: chunk,
                                flag: Some(name),
                                help: "invalid value",
                            },
                        });
                    }
                }
            }
        }

        Ok(None)
    }

    pub fn parse_required_flag<T>(
        &mut self,
        names: &'static [&'static str],
    ) -> Result<T, Error<'cmd>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        self.parse_optional_flag(names).and_then(|val| {
            val.ok_or(Error {
                line: self.line,
                kind: ErrorKind::FlagRequired { flags: names },
            })
        })
    }
}

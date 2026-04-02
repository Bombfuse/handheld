use handheld_cart::CartWriter;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 5 {
        eprintln!("Usage: cart-packer --name NAME --wasm FILE [--author AUTHOR] [--version VER] [--spritesheet FILE] -o OUTPUT");
        std::process::exit(1);
    }

    let mut name = String::new();
    let mut author = String::from("Unknown");
    let mut version = String::from("1.0");
    let mut wasm_path = PathBuf::new();
    let mut sprite_path: Option<PathBuf> = None;
    let mut output_path = PathBuf::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => { i += 1; name = args[i].clone(); }
            "--author" => { i += 1; author = args[i].clone(); }
            "--version" => { i += 1; version = args[i].clone(); }
            "--wasm" => { i += 1; wasm_path = PathBuf::from(&args[i]); }
            "--spritesheet" => { i += 1; sprite_path = Some(PathBuf::from(&args[i])); }
            "-o" => { i += 1; output_path = PathBuf::from(&args[i]); }
            _ => { eprintln!("Unknown arg: {}", args[i]); std::process::exit(1); }
        }
        i += 1;
    }

    if name.is_empty() || wasm_path.as_os_str().is_empty() || output_path.as_os_str().is_empty() {
        eprintln!("--name, --wasm, and -o are required");
        std::process::exit(1);
    }

    // Read WASM, optionally optimize with wasm-opt
    let wasm_data = fs::read(&wasm_path).expect("Failed to read WASM file");
    let wasm_data = try_optimize_wasm(&wasm_data, &wasm_path);

    let mut writer = CartWriter::new();
    writer.set_meta(&name, &author, &version);
    writer.set_wasm(&wasm_data);

    if let Some(ref sp) = sprite_path {
        let sprites = fs::read(sp).expect("Failed to read spritesheet");
        writer.set_spritesheet(&sprites);
    }

    let cart = writer.build();
    fs::write(&output_path, &cart).expect("Failed to write cart file");

    println!("Packed: {} ({} bytes)", output_path.display(), cart.len());
    println!("  Name: {name}");
    println!("  WASM: {} bytes", wasm_data.len());
    if let Some(ref sp) = sprite_path {
        let size = fs::metadata(sp).map(|m| m.len()).unwrap_or(0);
        println!("  Spritesheet: {size} bytes");
    }
}

fn try_optimize_wasm(data: &[u8], path: &PathBuf) -> Vec<u8> {
    if Command::new("wasm-opt").arg("--version").output().is_err() {
        eprintln!("wasm-opt not found, skipping optimization");
        return data.to_vec();
    }

    let tmp_in = std::env::temp_dir().join("cart_packer_in.wasm");
    let tmp_out = std::env::temp_dir().join("cart_packer_out.wasm");
    fs::write(&tmp_in, data).unwrap();

    let status = Command::new("wasm-opt")
        .args(["-Oz", "--remove-unused-module-elements"])
        .arg(&tmp_in)
        .arg("-o")
        .arg(&tmp_out)
        .status();

    match status {
        Ok(s) if s.success() => {
            let optimized = fs::read(&tmp_out).unwrap();
            eprintln!("wasm-opt: {} -> {} bytes", data.len(), optimized.len());
            let _ = fs::remove_file(&tmp_in);
            let _ = fs::remove_file(&tmp_out);
            optimized
        }
        _ => {
            eprintln!("wasm-opt failed, using unoptimized WASM");
            let _ = fs::remove_file(&tmp_in);
            data.to_vec()
        }
    }
}

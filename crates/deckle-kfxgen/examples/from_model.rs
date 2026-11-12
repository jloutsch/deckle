// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo run --release -p deckle-kfxgen --example from_model -- <model_dir> <out.kfx>`: a dumped
//! kfxgen model into KFX, printing how long `generate` took (loading and writing not counted).
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, out] = args.as_slice() else {
        eprintln!("usage: from_model <model_dir> <out.kfx>");
        std::process::exit(2);
    };
    let model = deckle_kfxgen::load::load_model(std::path::Path::new(dir)).unwrap_or_else(|e| {
        eprintln!("{dir}: {e}");
        std::process::exit(1);
    });
    let start = Instant::now();
    let kfx = deckle_kfxgen::generate::generate(&model).unwrap_or_else(|e| {
        eprintln!("{dir}: {e}");
        std::process::exit(1);
    });
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    std::fs::write(out, kfx.bytes).expect("write the KFX");
    println!("{ms:.1} ms");
}

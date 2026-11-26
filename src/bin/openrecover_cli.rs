use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use openrecover::core::device::{
    enumerate_devices, inspect_image_file, is_same_volume, open_device_reader,
};
use openrecover::core::recover::recover_entries;
use openrecover::core::run_scan;
use openrecover::core::types::{format_bytes, RecoverEvent, ScanEvent, ScanMode};

fn print_usage() {
    println!("OpenRecover CLI v0.1.0");
    println!("Usage:");
    println!("  openrecover-cli list");
    println!("  openrecover-cli scan <source> [--mode quick|carve|combined] [--filter <substr>] [--out <dest_dir>]");
    println!("Examples:");
    println!("  openrecover-cli list");
    println!("  openrecover-cli scan C: --mode quick --filter AppData/Roaming --out D:\\Restored");
    println!("  openrecover-cli scan sdcard.img --mode combined --out D:\\SD_Out");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }

    match args[1].as_str() {
        "list" => {
            let devs = enumerate_devices();
            for d in devs {
                println!(
                    "{:<18} | {:<18} | {:<7} | {}",
                    d.id,
                    d.bus_kind.label(false),
                    d.fs_kind,
                    d.display_name
                );
            }
        }
        "scan" => {
            if args.len() < 3 {
                print_usage();
                std::process::exit(1);
            }
            let src = &args[2];
            let mut mode = ScanMode::QuickTree;
            let mut filter: Option<String> = None;
            let mut out_dir: Option<PathBuf> = None;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--mode" if i + 1 < args.len() => {
                        mode = match args[i + 1].as_str() {
                            "carve" => ScanMode::DeepCarve,
                            "combined" => ScanMode::Combined,
                            _ => ScanMode::QuickTree,
                        };
                        i += 2;
                    }
                    "--filter" if i + 1 < args.len() => {
                        filter = Some(args[i + 1].to_ascii_lowercase().replace('\\', "/"));
                        i += 2;
                    }
                    "--out" if i + 1 < args.len() => {
                        out_dir = Some(PathBuf::from(&args[i + 1]));
                        i += 2;
                    }
                    _ => i += 1,
                }
            }

            let device = {
                let p = PathBuf::from(src);
                if p.exists() && p.is_file() {
                    let imgs = inspect_image_file(&p).expect("Failed to open disk image");
                    imgs.into_iter().next().expect("No partitions or volume in image")
                } else {
                    let devs = enumerate_devices();
                    let norm = src.trim_end_matches(['\\', '/']).to_ascii_lowercase();
                    devs.into_iter()
                        .find(|d| {
                            d.id.to_ascii_lowercase() == norm
                                || d.raw_device_path.to_ascii_lowercase() == norm
                                || d.mount_point
                                    .as_deref()
                                    .unwrap_or("")
                                    .trim_end_matches(['\\', '/'])
                                    .to_ascii_lowercase()
                                    == norm
                        })
                        .unwrap_or_else(|| panic!("Device not found: {}", src))
                }
            };

            println!("Scanning {} ({:?})...", device.display_name, mode);
            let cancel = Arc::new(AtomicBool::new(false));
            let res = run_scan(&device, mode, &cancel, &mut |ev| {
                if let ScanEvent::Progress {
                    stage,
                    fraction,
                    found_count,
                    detail,
                } = ev
                {
                    eprintln!(
                        "[{:>12}] {:>5.1}% | found={} | {}",
                        stage,
                        fraction * 100.0,
                        found_count,
                        detail
                    );
                }
            });

            match res {
                ScanEvent::Failed(err) => {
                    eprintln!("Error: {}", err);
                    std::process::exit(2);
                }
                ScanEvent::Finished {
                    entries,
                    elapsed_secs,
                    warnings,
                } => {
                    for w in &warnings {
                        eprintln!("Note: {}", w);
                    }
                    let matched: Vec<_> = entries
                        .into_iter()
                        .filter(|e| match &filter {
                            None => true,
                            Some(f) => e.rel_path_lower.contains(f),
                        })
                        .collect();
                    let total_bytes: u64 = matched.iter().map(|e| e.size).sum();
                    println!(
                        "Done in {:.2}s: {} matched files ({})",
                        elapsed_secs,
                        matched.len(),
                        format_bytes(total_bytes)
                    );

                    if let Some(dest) = out_dir {
                        if is_same_volume(&device, &dest) {
                            eprintln!(
                                "Refusing to recover onto the same source volume: {}",
                                dest.display()
                            );
                            std::process::exit(3);
                        }
                        let dev = open_device_reader(&device).expect("Cannot open reader");
                        let sum = recover_entries(&dev, &matched, &dest, true, &cancel, &mut |ev| {
                            if let RecoverEvent::Progress {
                                done_files,
                                total_files,
                                bytes_written,
                                ..
                            } = ev
                            {
                                eprintln!(
                                    "Recovering {}/{} ({})",
                                    done_files,
                                    total_files,
                                    format_bytes(bytes_written)
                                );
                            }
                        });
                        println!(
                            "Recovered: intact={}, zero={}, trimmed_sep={}, err={}, bytes={}, dest={}",
                            sum.recovered_intact,
                            sum.recovered_zero_size,
                            sum.separated_trimmed,
                            sum.skipped_errors,
                            format_bytes(sum.bytes_written),
                            sum.dest_dir
                        );
                    } else {
                        for e in matched.iter().take(30) {
                            println!(
                                "  [{}] {:>10} | {} | {}",
                                e.health.label(false),
                                format_bytes(e.size),
                                e.source_desc,
                                e.rel_path
                            );
                        }
                        if matched.len() > 30 {
                            println!("  ... and {} more", matched.len() - 30);
                        }
                    }
                }
                _ => {}
            }
        }
        _ => print_usage(),
    }
}

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::{env, path::Path};

mod settings;
mod ui;

fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.len() == 1 {
        if let Err(error) = ui::run() {
            eprintln!("Arayüz açılamadı: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() < 3 {
        eprintln!(
            "Kullanım: spotlite status|pause|resume|next|previous <Soloist-veri-klasörü> | spotlite play <Soloist-veri-klasörü> <Spotify-bağlantısı> | spotlite local <dosya.mp3> | spotlite library <Spotify-Client-ID>"
        );
        std::process::exit(2);
    }
    if args[1] == "play" && args.len() == 4 {
        let uri = args[3].to_str().and_then(spotlite::spotify_uri);
        let Some(uri) = uri else {
            eprintln!("Spotify bağlantısı geçersiz");
            std::process::exit(2);
        };
        let result = spotlite::read_endpoint(Path::new(&args[2])).and_then(|endpoint| {
            spotlite::send_command(endpoint, spotlite::Command::Play(&uri)).map_err(io_error)
        });
        match result {
            Ok(()) => println!("Komut kabul edildi; oynatma durumu ayrıca doğrulanmalı."),
            Err(error) => {
                eprintln!("Komut gönderilemedi: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() != 3 {
        eprintln!("Geçersiz komut");
        std::process::exit(2);
    }
    let command = match args[1].to_str() {
        Some("pause") => Some(spotlite::Command::Pause),
        Some("resume") => Some(spotlite::Command::Resume),
        Some("next") => Some(spotlite::Command::Next),
        Some("previous") => Some(spotlite::Command::Previous),
        _ => None,
    };
    if let Some(command) = command {
        let result = spotlite::read_endpoint(Path::new(&args[2]))
            .and_then(|endpoint| spotlite::send_command(endpoint, command).map_err(io_error));
        match result {
            Ok(()) => println!("Komut kabul edildi; oynatma durumu ayrıca doğrulanmalı."),
            Err(error) => {
                eprintln!("Komut gönderilemedi: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args[1] == "local" {
        if let Err(error) = spotlite::play_local_mp3(Path::new(&args[2])) {
            eprintln!("Yerel dosya çalınamadı: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args[1] == "library" {
        let Some(client_id) = args[2].to_str() else {
            eprintln!("Client ID okunamadı");
            std::process::exit(2);
        };
        match spotlite::login::authorize(client_id) {
            Ok(token) => match spotlite::library::playlists(token.as_str(), 0) {
                Ok(page) => {
                    for playlist in page.playlists {
                        println!("{} — {}", playlist.name, playlist.uri);
                    }
                    if page.next_offset.is_some() {
                        println!("Daha fazla liste var; sayfalama henüz arayüze bağlanmadı.");
                    }
                }
                Err(error) => {
                    eprintln!("Kitaplık alınamadı: {error}");
                    std::process::exit(1);
                }
            },
            Err(error) => {
                eprintln!("Giriş yapılamadı: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args[1] != "status" {
        eprintln!("Bilinmeyen komut");
        std::process::exit(2);
    }
    match spotlite::read_endpoint(Path::new(&args[2]))
        .and_then(|address| spotlite::read_status(address).map_err(io_error))
    {
        Ok(state) => {
            println!(
                "Spotify oturumu: {}",
                if state.logged_in {
                    "bağlı"
                } else {
                    "bağlı değil"
                }
            );
            if let Some(status) = state.status {
                println!("Oynatma: {status}");
            }
            if let Some(title) = state.title {
                println!("Parça: {title}");
            }
        }
        Err(error) => {
            eprintln!("Soloist durumuna ulaşılamadı: {error}");
            std::process::exit(1);
        }
    }
}

fn io_error(error: Box<dyn std::error::Error>) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

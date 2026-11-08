use std::{env, path::Path};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 3 {
        eprintln!("Kullanım: spotlite status <Soloist-veri-klasörü> | spotlite local <dosya.mp3>");
        std::process::exit(2);
    }
    if args[1] == "local" {
        if let Err(error) = spotlite::play_local_mp3(Path::new(&args[2])) {
            eprintln!("Yerel dosya çalınamadı: {error}");
            std::process::exit(1);
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

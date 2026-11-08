//! Small native test interface.

use eframe::egui::{self, Color32, RichText};
use std::{path::PathBuf, sync::mpsc, thread, time::Duration};

enum AudioCommand {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
}
enum Event {
    Message(String),
    LoginFailed(String),
    LocalPlaying(String),
    LocalStopped,
    Library(Vec<spotlite::library::Playlist>, bool),
    SpotifyStatus(spotlite::PlaybackStatus),
    Background(Result<(PathBuf, egui::ColorImage), String>),
    SettingsSaved(Result<(), String>),
}

struct Prototype {
    songs: Vec<PathBuf>,
    playlists: Vec<spotlite::library::Playlist>,
    client_id: String,
    soloist_dir: String,
    spotify_link: String,
    message: String,
    local_title: Option<String>,
    local_paused: bool,
    login_busy: bool,
    events: mpsc::Receiver<Event>,
    event_sender: mpsc::Sender<Event>,
    audio: mpsc::Sender<AudioCommand>,
    accent: Color32,
    background_path: Option<PathBuf>,
    background: Option<egui::TextureHandle>,
    saving: bool,
}

impl Prototype {
    fn new(ctx: &egui::Context) -> Self {
        ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(23.0));
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        ctx.set_style_of(egui::Theme::Dark, style);
        let (event_sender, events) = mpsc::channel();
        let (audio, audio_commands) = mpsc::channel();
        let audio_events = event_sender.clone();
        let repaint = ctx.clone();
        thread::spawn(move || audio_worker(audio_commands, audio_events, repaint));
        let (settings, message) = match crate::settings::path().map(|p| crate::settings::load(&p)) {
            Some(Ok(settings)) => (
                settings,
                "Deneme sürümü: Spotify ve yerel müzik bağlantılarını test edin.".to_owned(),
            ),
            Some(Err(error)) => (
                crate::settings::Settings::default(),
                format!("Ayarlar okunamadı: {error}"),
            ),
            None => (
                crate::settings::Settings::default(),
                "Ayar klasörü bulunamadı.".to_owned(),
            ),
        };
        if let Some(path) = settings.background.clone() {
            load_background(path, event_sender.clone(), ctx.clone());
        }
        Self {
            songs: Vec::new(),
            playlists: Vec::new(),
            client_id: settings.client_id,
            soloist_dir: settings.soloist_dir,
            spotify_link: String::new(),
            message,
            local_title: None,
            local_paused: false,
            login_busy: false,
            events,
            event_sender,
            audio,
            accent: Color32::from_rgb(settings.accent[0], settings.accent[1], settings.accent[2]),
            background_path: settings.background,
            background: None,
            saving: false,
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Message(message) => {
                    self.message = message;
                }
                Event::LoginFailed(message) => {
                    self.message = message;
                    self.login_busy = false;
                }
                Event::LocalPlaying(title) => {
                    self.local_title = Some(title);
                    self.local_paused = false;
                    self.message = "Yerel müzik oynatılıyor.".into();
                }
                Event::LocalStopped => {
                    self.local_title = None;
                    self.local_paused = false;
                    self.message = "Yerel oynatma sona erdi.".into();
                }
                Event::Library(playlists, more) => {
                    self.playlists = playlists;
                    self.login_busy = false;
                    self.message = if more {
                        "İlk 50 liste yüklendi; devamı bu prototipte henüz gösterilmiyor."
                    } else {
                        "Spotify kitaplığı yüklendi."
                    }
                    .into();
                }
                Event::SpotifyStatus(state) => {
                    self.message = format!(
                        "Spotify: {} · {}",
                        if state.logged_in {
                            "bağlı"
                        } else {
                            "giriş gerekli"
                        },
                        state.status.unwrap_or_else(|| "durum bilinmiyor".into())
                    );
                }
                Event::Background(result) => match result {
                    Ok((path, image)) => {
                        self.background = Some(ctx.load_texture(
                            "background",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.background_path = Some(path);
                        self.message =
                            "Arka plan yüklendi; kalıcı olması için ayarları kaydedin.".into();
                    }
                    Err(error) => self.message = format!("Arka plan yüklenemedi: {error}"),
                },
                Event::SettingsSaved(result) => {
                    self.saving = false;
                    self.message = match result {
                        Ok(()) => "Ayarlar kaydedildi.".into(),
                        Err(error) => format!("Ayarlar kaydedilemedi: {error}"),
                    };
                }
            }
        }
    }

    fn save_settings(&mut self, ctx: &egui::Context) {
        let Some(path) = crate::settings::path() else {
            self.message = "Ayar klasörü bulunamadı.".into();
            return;
        };
        let settings = crate::settings::Settings {
            accent: [self.accent.r(), self.accent.g(), self.accent.b()],
            background: self.background_path.clone(),
            client_id: self.client_id.clone(),
            soloist_dir: self.soloist_dir.clone(),
        };
        let tx = self.event_sender.clone();
        let ctx = ctx.clone();
        self.saving = true;
        thread::spawn(move || {
            let result = crate::settings::save(&path, &settings);
            let _ = tx.send(Event::SettingsSaved(result));
            ctx.request_repaint();
        });
    }

    fn login(&mut self, ctx: &egui::Context) {
        let client_id = self.client_id.trim().to_owned();
        let tx = self.event_sender.clone();
        let repaint = ctx.clone();
        self.login_busy = true;
        self.message = "Spotify yetkilendirmesi bekleniyor…".into();
        thread::spawn(move || {
            let event = match spotlite::login::authorize(&client_id) {
                Ok(token) => match spotlite::library::playlists(token.as_str(), 0) {
                    Ok(page) => Event::Library(page.playlists, page.next_offset.is_some()),
                    Err(error) => Event::LoginFailed(format!("Kitaplık alınamadı: {error}")),
                },
                Err(error) => Event::LoginFailed(format!("Giriş tamamlanamadı: {error}")),
            };
            let _ = tx.send(event);
            repaint.request_repaint();
        });
    }

    fn spotify(&self, ctx: &egui::Context, uri: Option<String>) {
        let directory = PathBuf::from(self.soloist_dir.trim());
        let tx = self.event_sender.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let event = match spotlite::read_endpoint(&directory) {
                Ok(endpoint) => match uri {
                    Some(uri) => {
                        match spotlite::send_command(endpoint, spotlite::Command::Play(&uri)) {
                            Ok(()) => Event::Message(
                                "Oynatma isteği kabul edildi; durumu kontrol edebilirsiniz.".into(),
                            ),
                            Err(error) => Event::Message(format!("İstek gönderilemedi: {error}")),
                        }
                    }
                    None => match spotlite::read_status(endpoint) {
                        Ok(state) => Event::SpotifyStatus(state),
                        Err(error) => Event::Message(format!("Soloist okunamadı: {error}")),
                    },
                },
                Err(error) => Event::Message(format!("Soloist klasörü okunamadı: {error}")),
            };
            let _ = tx.send(event);
            repaint.request_repaint();
        });
    }

    fn spotify_repeat(&self, ctx: &egui::Context, mode: spotlite::RepeatMode) {
        let directory = PathBuf::from(self.soloist_dir.trim());
        let tx = self.event_sender.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = spotlite::read_endpoint(&directory).and_then(|endpoint| {
                spotlite::set_repeat_mode(endpoint, mode)
                    .map_err(|error| std::io::Error::other(error.to_string()))
            });
            let event = match result {
                Ok(()) => Event::Message(
                    "Tekrar isteği kabul edildi; gerçek durum Spotify'dan doğrulanmalı.".into(),
                ),
                Err(error) => Event::Message(format!("Tekrar isteği tamamlanamadı: {error}")),
            };
            let _ = tx.send(event);
            repaint.request_repaint();
        });
    }
}

impl eframe::App for Prototype {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        egui::Panel::bottom("player").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(
                        self.local_title
                            .as_deref()
                            .unwrap_or("Yerel parça seçilmedi"),
                    )
                    .strong(),
                );
                if ui
                    .add_enabled(
                        self.local_title.is_some(),
                        egui::Button::new(if self.local_paused {
                            "Devam"
                        } else {
                            "Duraklat"
                        }),
                    )
                    .clicked()
                {
                    let command = if self.local_paused {
                        AudioCommand::Resume
                    } else {
                        AudioCommand::Pause
                    };
                    if self.audio.send(command).is_ok() {
                        self.local_paused = !self.local_paused;
                    }
                }
                if ui
                    .add_enabled(self.local_title.is_some(), egui::Button::new("Durdur"))
                    .clicked()
                {
                    let _ = self.audio.send(AudioCommand::Stop);
                }
            });
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(texture) = &self.background {
                ui.painter().image(texture.id(), ui.max_rect(), egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::from_white_alpha(65));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading(RichText::new("SpotLite").color(self.accent));
                ui.label(RichText::new("Yerel prototip").weak());
                ui.add_space(12.0);
                ui.label(&self.message);
                egui::CollapsingHeader::new("Görünüm ve ayarlar").show(ui, |ui| {
                    ui.horizontal(|ui| { ui.label("Vurgu rengi"); ui.color_edit_button_srgba(&mut self.accent); });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Arka plan seç").clicked() && let Some(path) = rfd::FileDialog::new().add_filter("Görsel", &["png", "jpg", "jpeg", "webp"]).pick_file() {
                            load_background(path, self.event_sender.clone(), ctx.clone());
                        }
                        if ui.button("Arka planı kaldır").clicked() { self.background = None; self.background_path = None; }
                        if ui.add_enabled(!self.saving, egui::Button::new("Ayarları kaydet")).clicked() { self.save_settings(&ctx); }
                    });
                    ui.label(RichText::new("Renk, arka plan yolu, Client ID ve Soloist klasörü saklanır. Parola ve erişim anahtarı saklanmaz.").weak());
                });
                ui.separator();
                ui.heading("Spotify kitaplığı");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Client ID");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.client_id)
                            .desired_width(300.0)
                            .hint_text("Spotify geliştirici uygulaması"),
                    );
                    if ui
                        .add_enabled(!self.login_busy, egui::Button::new("Spotify ile giriş"))
                        .clicked()
                    {
                        self.login(&ctx);
                    }
                });
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Soloist klasörü");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.soloist_dir)
                            .desired_width(300.0)
                            .hint_text("ws.port içeren klasör"),
                    );
                    if ui.button("Seç").clicked()
                        && let Some(folder) = rfd::FileDialog::new().pick_folder()
                    {
                        self.soloist_dir = folder.to_string_lossy().into_owned();
                    }
                    if ui.button("Bağlantıyı kontrol et").clicked() {
                        self.spotify(&ctx, None);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.spotify_link)
                            .desired_width(350.0)
                            .hint_text("Spotify parça veya playlist bağlantısı"),
                    );
                    if ui.button("Spotify'da başlat").clicked() {
                        if let Some(uri) = spotlite::spotify_uri(&self.spotify_link) {
                            self.spotify(&ctx, Some(uri));
                        } else {
                            self.message = "Spotify bağlantısı geçersiz.".into();
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Tekrar:");
                    if ui.button("Kapalı").clicked() {
                        self.spotify_repeat(&ctx, spotlite::RepeatMode::Off);
                    }
                    if ui.button("Liste").clicked() {
                        self.spotify_repeat(&ctx, spotlite::RepeatMode::Context);
                    }
                    if ui.button("Şarkı").clicked() {
                        self.spotify_repeat(&ctx, spotlite::RepeatMode::Track);
                    }
                });
                for playlist in &self.playlists {
                    if ui.selectable_label(false, &playlist.name).clicked() {
                        self.spotify(&ctx, Some(playlist.uri.clone()));
                    }
                }
                ui.add_space(14.0);
                ui.separator();
                ui.heading("Yerel müzik");
                if ui.button("MP3 ekle").clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter("MP3", &["mp3"])
                        .pick_files()
                {
                    for file in files {
                        if !self.songs.contains(&file) {
                            self.songs.push(file);
                        }
                    }
                }
                ui.label(
                    RichText::new(
                        "Spotify çalıyorsa yerel dosya başlatmak iki sesi birlikte duyurabilir.",
                    )
                    .weak(),
                );
                for path in &self.songs {
                    let title = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "MP3".into());
                    if ui.selectable_label(false, title).clicked() {
                        let _ = self.audio.send(AudioCommand::Play(path.clone()));
                    }
                }
            });
        });
    }
}

fn load_background(path: PathBuf, tx: mpsc::Sender<Event>, ctx: egui::Context) {
    thread::spawn(move || {
        let result = (|| -> Result<_, String> {
            let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            if file.metadata().map_err(|e| e.to_string())?.len() > 12 * 1024 * 1024 {
                return Err("Görsel 12 MB sınırını aşıyor".into());
            }
            let mut reader = image::ImageReader::new(std::io::BufReader::new(file))
                .with_guessed_format()
                .map_err(|e| e.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let image = reader
                .decode()
                .map_err(|e| e.to_string())?
                .thumbnail(1920, 1080)
                .to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            Ok((
                path,
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
            ))
        })();
        let _ = tx.send(Event::Background(result));
        ctx.request_repaint();
    });
}

fn audio_worker(rx: mpsc::Receiver<AudioCommand>, tx: mpsc::Sender<Event>, ctx: egui::Context) {
    let mut stream: Option<rodio::OutputStream> = None;
    let mut sink: Option<rodio::Sink> = None;
    loop {
        let command = if sink.as_ref().is_some_and(|s| !s.is_paused()) {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if sink.as_ref().is_some_and(rodio::Sink::empty) {
                        sink = None;
                        stream = None;
                        let _ = tx.send(Event::LocalStopped);
                        ctx.request_repaint();
                    }
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match command {
            Some(AudioCommand::Play(path)) => {
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let file = std::fs::File::open(&path)?;
                    let decoder = rodio::Decoder::try_from(file)?;
                    if stream.is_none() {
                        stream = Some(rodio::OutputStreamBuilder::open_default_stream()?);
                    }
                    let next = rodio::Sink::connect_new(
                        stream.as_ref().ok_or("Ses aygıtı bulunamadı")?.mixer(),
                    );
                    next.append(decoder);
                    if let Some(previous) = sink.replace(next) {
                        previous.stop();
                    }
                    Ok(())
                })();
                let event = match result {
                    Ok(()) => Event::LocalPlaying(
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "MP3".into()),
                    ),
                    Err(error) => Event::Message(format!("MP3 açılamadı: {error}")),
                };
                let _ = tx.send(event);
                ctx.request_repaint();
            }
            Some(AudioCommand::Pause) => {
                if let Some(s) = &sink {
                    s.pause();
                }
            }
            Some(AudioCommand::Resume) => {
                if let Some(s) = &sink {
                    s.play();
                }
            }
            Some(AudioCommand::Stop) => {
                if let Some(s) = sink.take() {
                    s.stop();
                }
                stream = None;
                let _ = tx.send(Event::LocalStopped);
                ctx.request_repaint();
            }
            None => {}
        }
    }
}

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 680.0])
            .with_min_inner_size([540.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SpotLite — Prototip",
        options,
        Box::new(|cc| Ok(Box::new(Prototype::new(&cc.egui_ctx)))),
    )
}

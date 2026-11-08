# SpotLite

SpotLite, düşük kaynak kullanımını ve kişiselleştirilebilir bir müzik deneyimini hedefleyen bağımsız bir Rust projesidir.

## Prototip durumu

Şu anda çalışmakta olan Spotify Soloist'in yerel bağlantısından giriş ve oynatma durumunu okuyan bir komut satırı prototipi ve henüz kullanıcı arayüzüne bağlanmamış temel oynatma komutları vardır. Tek bir yerel MP3 dosyası komut satırından çalınabilir. Grafik arayüzü, Spotify kurulumu ve kitaplık yönetimi henüz uygulanmadı.

```powershell
cargo test
cargo run -- status "SOLOIST_VERI_KLASORU"
cargo run -- local "sarki.mp3"
```

Soloist için kendi Spotify anahtarınız ve WebSocket API'si etkin çalışan bir kurulum gerekir. `status` komutu yalnızca yerel bağlantı üzerinden okuma yapar ve oynatma komutu göndermez. Soloist ayrı indirilir; bu depoya dahil edilmez. Spotify kurulumu ve kullanım koşulları için [resmî belgelere](https://developer.spotify.com/documentation/soloist) bakın.

Durum okuma, gerçek bir çalışan Soloist oturumunda test edildi. Yerel oynatma, Windows'ta kısa ve sessiz bir MP3 test dosyasıyla doğrulandı. Komut gönderimi yalnızca yerel test sunucusunda sınandı; Soloist'in komut kabulü, oynatmanın tamamlandığını tek başına garanti etmez.

Mimari ve protokol hakkında [Soloist WebSocket belgeleri](https://developer.spotify.com/documentation/soloist/reference/websocket-api) incelenmiştir. Bu kod, Spotifast uygulamasından kopyalanmamıştır.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub accent: [u8; 3],
    pub background: Option<PathBuf>,
    pub client_id: String,
    pub soloist_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: [98, 227, 155],
            background: None,
            client_id: String::new(),
            soloist_dir: String::new(),
        }
    }
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("SpotLite").join("settings.json"))
}

pub fn load(path: &Path) -> Result<Settings, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut contents = String::new();
    std::io::Read::by_ref(&mut file)
        .take(65_537)
        .read_to_string(&mut contents)
        .map_err(|e| e.to_string())?;
    if contents.len() > 65_536 {
        return Err("Ayar dosyası çok büyük".into());
    }
    let value: serde_json::Value = serde_json::from_str(&contents).map_err(|e| e.to_string())?;
    let accent = value
        .get("accent")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 3)
        .and_then(|a| {
            Some([
                u8::try_from(a[0].as_u64()?).ok()?,
                u8::try_from(a[1].as_u64()?).ok()?,
                u8::try_from(a[2].as_u64()?).ok()?,
            ])
        })
        .unwrap_or(Settings::default().accent);
    Ok(Settings {
        accent,
        background: value
            .get("background")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
        client_id: value
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        soloist_dir: value
            .get("soloist_dir")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    let parent = path.parent().ok_or("Ayar klasörü bulunamadı")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let data = serde_json::json!({"accent": settings.accent, "background": settings.background, "client_id": settings.client_id, "soloist_dir": settings.soloist_dir});
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut temp, &data).map_err(|e| e.to_string())?;
    temp.flush().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_roundtrip_and_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = Settings {
            client_id: "public-client-id".into(),
            ..Settings::default()
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        s.accent = [1, 2, 3];
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
    }
}

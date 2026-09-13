use std::{fs,io::{self,Write},path::Path,time::Duration};
pub fn append(root:&Path,name:&str,line:&str)->io::Result<()> {
    append_limited(root,name,line,1024*1024)
}
fn append_limited(root:&Path,name:&str,line:&str,limit:u64)->io::Result<()> {
    let _guard=crate::acquire_process_file_lock(root.join(format!("{name}.lock")),Duration::from_secs(1))?;
    let path=root.join(name);
    if fs::metadata(&path).map(|m|m.len()).unwrap_or(0)+line.len() as u64>limit {
        let oldest=root.join(format!("{name}.3"));
        if oldest.exists(){fs::remove_file(oldest)?;}
        for n in (1..3).rev(){let from=root.join(format!("{name}.{n}"));if from.exists(){fs::rename(from,root.join(format!("{name}.{}",n+1)))?;}}
        if path.exists(){fs::rename(&path,root.join(format!("{name}.1")))?;}
    }
    let mut f=fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())
}
#[cfg(test)] mod tests {
    #[test] fn rotation_is_bounded_and_preserves_latest() {
        let root=std::env::temp_dir().join(format!("spotlite-log-test-{}-{}",std::process::id(),crate::now_ms()));
        std::fs::create_dir(&root).unwrap();
        for _ in 0..10 {super::append_limited(&root,"test.log","12345678\n",10).unwrap();}
        assert_eq!(std::fs::read_to_string(root.join("test.log")).unwrap(),"12345678\n");
        assert!(root.join("test.log.3").exists());assert!(!root.join("test.log.4").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

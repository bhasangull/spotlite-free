//! Window lifecycle owned by SpotLite, not global desktop automation. A parked
//! account retains its real viewport and has no taskbar/Alt-Tab entry.
#[cfg(windows)] mod platform {
    use std::{collections::HashMap,ffi::c_void,sync::{Mutex,OnceLock}};
    #[derive(Clone,Copy,Default)] #[repr(C)] struct Rect {left:i32,top:i32,right:i32,bottom:i32}
    #[derive(Clone,Copy)] struct Saved {rect:Rect,style:isize}
    static SAVED:OnceLock<Mutex<HashMap<isize,Saved>>>=OnceLock::new();
    #[link(name="user32")] extern "system" {
        fn GetWindowRect(hwnd:*mut c_void,rect:*mut Rect)->i32;
        fn GetWindowLongPtrW(hwnd:*mut c_void,index:i32)->isize;
        fn SetWindowLongPtrW(hwnd:*mut c_void,index:i32,value:isize)->isize;
        fn SetWindowPos(hwnd:*mut c_void,after:*mut c_void,x:i32,y:i32,w:i32,h:i32,flags:u32)->i32;
        fn ShowWindow(hwnd:*mut c_void,command:i32)->i32;
        fn GetForegroundWindow()->*mut c_void;
        fn IsWindowVisible(hwnd:*mut c_void)->i32;
        fn IsIconic(hwnd:*mut c_void)->i32;
        fn GetWindowThreadProcessId(hwnd:*mut c_void,pid:*mut u32)->u32;
    }
    pub fn process(window:isize)->u32 {let mut pid=0;unsafe{GetWindowThreadProcessId(window as *mut c_void,&mut pid);}pid}
    pub fn process_birth(pid:u32)->Option<u64> {
        #[link(name="kernel32")] extern "system" {
            fn OpenProcess(access:u32,inherit:i32,pid:u32)->*mut c_void;
            fn GetProcessTimes(handle:*mut c_void,creation:*mut u64,exit:*mut u64,kernel:*mut u64,user:*mut u64)->i32;
            fn CloseHandle(handle:*mut c_void)->i32;
        }
        unsafe {let h=OpenProcess(0x1000,0,pid);if h.is_null(){return None;}
            let (mut born,mut exited,mut kernel,mut user)=(0,0,0,0);
            let ok=GetProcessTimes(h,&mut born,&mut exited,&mut kernel,&mut user)!=0;CloseHandle(h);
            ok.then_some(born)
        }
    }
    pub fn background(window:isize,park:bool) {
        let hwnd=window as *mut c_void;
        let mut saved=SAVED.get_or_init(Default::default).lock().unwrap();
        unsafe {
            if park {
                if saved.contains_key(&window){return;}
                let mut rect=Rect::default();if GetWindowRect(hwnd,&mut rect)==0{return;}
                let style=GetWindowLongPtrW(hwnd,-20);
                // Window may have been initially launched off-screen to avoid
                // a visible flash. Give explicit Open/Login a useful restore rect.
                let restore=if rect.left < -10000 {Rect{left:100,top:100,right:660,bottom:750}}else{rect};
                saved.insert(window,Saved{rect:restore,style});
                ShowWindow(hwnd,0);
                SetWindowLongPtrW(hwnd,-20,(style|0x80)&!0x40000);
                SetWindowPos(hwnd,std::ptr::null_mut(),-20000,-20000,560,650,0x4|0x10|0x20);
                ShowWindow(hwnd,4); // SW_SHOWNOACTIVATE; not minimized or 1x1.
            } else if let Some(old)=saved.remove(&window) {
                ShowWindow(hwnd,0);SetWindowLongPtrW(hwnd,-20,old.style);
                SetWindowPos(hwnd,std::ptr::null_mut(),old.rect.left,old.rect.top,old.rect.right-old.rect.left,old.rect.bottom-old.rect.top,0x4|0x10|0x20);
                ShowWindow(hwnd,4);
            }
        }
    }
    pub fn state(window:isize)->serde_json::Value {
        let hwnd=window as *mut c_void;let mut r=Rect::default();
        unsafe {GetWindowRect(hwnd,&mut r);serde_json::json!({"parked":SAVED.get_or_init(Default::default).lock().unwrap().contains_key(&window),
            "foreground":GetForegroundWindow()==hwnd,"visible":IsWindowVisible(hwnd)!=0,"minimized":IsIconic(hwnd)!=0,
            "left":r.left,"top":r.top,"width":r.right-r.left,"height":r.bottom-r.top,"toolWindow":GetWindowLongPtrW(hwnd,-20)&0x80!=0})}
    }
    pub fn compact(window:isize,enabled:bool) {
        if SAVED.get_or_init(Default::default).lock().unwrap().contains_key(&window) {
            unsafe {SetWindowPos(window as *mut c_void,std::ptr::null_mut(),-20000,-20000,
                if enabled {420}else{560},if enabled {240}else{650},0x4|0x10);}
        }
    }
}
#[cfg(windows)] pub use platform::{background,state,process,process_birth,compact};
#[cfg(not(windows))] pub fn compact(_:isize,_:bool){}
#[cfg(not(windows))] pub fn process_birth(_:u32)->Option<u64>{None}
#[cfg(not(windows))] pub fn background(_:isize,_:bool){}
#[cfg(not(windows))] pub fn state(_:isize)->serde_json::Value{serde_json::Value::Null}
#[cfg(not(windows))] pub fn process(_:isize)->u32{0}

// Apply on the FIRST browser process (the panel) as well as each subsequent
// account: Chromium reuses that process for this dedicated user-data-dir.
pub fn browser_arguments()->&'static [&'static str] {
    &["--mute-audio","--autoplay-policy=no-user-gesture-required",
      "--disable-background-timer-throttling","--disable-renderer-backgrounding",
      "--disable-backgrounding-occluded-windows",
      "--disable-features=CalculateNativeWinOcclusion"]
}
#[cfg(test)] mod tests {
    #[test] fn silent_background_flags_do_not_weaken_drm_or_isolation(){let args=super::browser_arguments();
        for needed in ["--mute-audio","--disable-background-timer-throttling","--disable-backgrounding-occluded-windows"]{assert!(args.contains(&needed));}
        for arg in args {assert!(!arg.contains("disable-web-security"));assert!(!arg.contains("no-sandbox"));assert!(!arg.contains("disable-gpu"));}
    }
}

//! Optional per-account IPv6 routing. Existing shared-browser accounts stay as-is.
//! An account gets its own Edge user-data-dir and a fail-closed local SOCKS5
//! listener. Every upstream TCP connection binds to that account's IPv6 first.
use std::{collections::HashSet,fs,io::{self,Read,Write},net::{Ipv6Addr,SocketAddr,SocketAddrV6,TcpListener,TcpStream,ToSocketAddrs},path::{Path,PathBuf},thread,time::Duration};
use socket2::{Domain,Socket,Type};

const TEMPLATE:&str="# SpotLite Free IPv6 havuzu — her satıra bu bilgisayara atanmış bir genel IPv6 yazın.\n# Yeni hesaplar listedeki ilk boş adresi alır. Mevcut hesaplar değişmez.\n";
pub fn pool_path(root:&Path)->PathBuf {root.join("ipv6-addresses.txt")}
pub fn assignment_path(root:&Path,n:u8)->PathBuf {root.join(format!("AccountIPv6{n}.txt"))}
pub fn browser_path(root:&Path,n:u8)->PathBuf {root.join(format!("IPv6Browser{n}"))}
pub fn cache_path(root:&Path,n:u8)->PathBuf {root.join(format!("IPv6Cache{n}"))}
pub fn proxy_port(n:u8)->u16 {43216+u16::from(n)}
pub fn ensure_pool(root:&Path)->io::Result<()> {
    fs::create_dir_all(root)?;
    let path=pool_path(root);
    if !path.exists(){crate::auth::atomic_write(&path,TEMPLATE.as_bytes())?;}
    Ok(())
}
fn is_global(address:Ipv6Addr)->bool {address.segments()[0]&0xe000==0x2000}
fn parse_pool(content:&str)->io::Result<Vec<Ipv6Addr>> {
    let mut addresses=Vec::new();let mut seen=HashSet::new();
    for line in content.lines(){let value=line.split('#').next().unwrap_or("").trim();if value.is_empty(){continue;}
        let address=value.parse::<Ipv6Addr>().map_err(|_|io::Error::other("IPv6 havuzunda geçersiz adres var"))?;
        if !is_global(address)||!seen.insert(address){return Err(io::Error::other("IPv6 havuzunda genel veya benzersiz olmayan adres var"));}
        addresses.push(address);
    }
    Ok(addresses)
}
pub fn read_assignment(root:&Path,n:u8)->io::Result<Option<Ipv6Addr>> {
    let path=assignment_path(root,n);if !path.exists(){return Ok(None);}
    let value=fs::read_to_string(path)?;let address=value.trim().parse::<Ipv6Addr>().map_err(|_|io::Error::other("Hesap IPv6 kaydı bozuk"))?;
    if !is_global(address){return Err(io::Error::other("Hesap IPv6 kaydı genel adres değil"));}
    Ok(Some(address))
}
fn next_unused(pool:&[Ipv6Addr],used:&HashSet<Ipv6Addr>)->io::Result<Ipv6Addr> {
    pool.iter().copied().find(|address|!used.contains(address)).ok_or_else(||io::Error::other("IPv6 havuzunda boş adres yok"))
}
/// Called under the existing accounts.allocate.lock before making an account.
pub fn assign_new(root:&Path,n:u8)->io::Result<Option<Ipv6Addr>> {
    ensure_pool(root)?;
    let pool=parse_pool(&fs::read_to_string(pool_path(root))?)?;
    if pool.is_empty(){return Ok(None);}
    let mut used=HashSet::new();for other in 1..=9 {if let Some(address)=read_assignment(root,other)?{used.insert(address);}}
    let selected=next_unused(&pool,&used)?;
    validate_local(selected)?;
    crate::auth::atomic_write(&assignment_path(root,n),selected.to_string().as_bytes())?;
    Ok(Some(selected))
}
fn validate_local(source:Ipv6Addr)->io::Result<()> {
    let socket=Socket::new(Domain::IPV6,Type::STREAM,None)?;
    socket.bind(&SocketAddr::V6(SocketAddrV6::new(source,0,0,0)).into())
        .map_err(|_|io::Error::other("IPv6 adresi bu bilgisayara atanmış değil"))
}
fn connect_from(source:Ipv6Addr,remote:SocketAddr)->io::Result<TcpStream> {
    if !remote.is_ipv6(){return Err(io::Error::other("Hedefin IPv6 adresi yok"));}
    let socket=Socket::new(Domain::IPV6,Type::STREAM,None)?;
    socket.bind(&SocketAddr::V6(SocketAddrV6::new(source,0,0,0)).into())?;
    socket.connect_timeout(&remote.into(),Duration::from_secs(12))?;
    Ok(socket.into())
}
fn upstream(source:Ipv6Addr,host:&str,port:u16)->io::Result<TcpStream> {
    let addresses=(host,port).to_socket_addrs()?;
    let mut last=None;
    for remote in addresses.filter(SocketAddr::is_ipv6){match connect_from(source,remote){Ok(stream)=>return Ok(stream),Err(error)=>last=Some(error)}}
    Err(last.unwrap_or_else(||io::Error::other("Hedefin IPv6 adresi yok")))
}
fn handle(mut client:TcpStream,source:Ipv6Addr)->io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(15)))?;
    client.set_write_timeout(Some(Duration::from_secs(15)))?;
    let mut header=[0u8;2];client.read_exact(&mut header)?;
    if header[0]!=5 || header[1]==0 {return Err(io::Error::other("SOCKS sürümü geçersiz"));}
    let mut methods=vec![0;usize::from(header[1])];client.read_exact(&mut methods)?;
    if !methods.contains(&0){client.write_all(&[5,255])?;return Ok(());}
    client.write_all(&[5,0])?;
    let mut request=[0u8;4];client.read_exact(&mut request)?;
    if request[0]!=5 || request[1]!=1 || request[2]!=0 {return Err(io::Error::other("SOCKS CONNECT gerekli"));}
    let host=match request[3] {
        3=>{let mut size=[0];client.read_exact(&mut size)?;if size[0]==0{return Err(io::Error::other("Boş hedef"));}
            let mut bytes=vec![0;usize::from(size[0])];client.read_exact(&mut bytes)?;
            String::from_utf8(bytes).map_err(|_|io::Error::other("Geçersiz hedef"))?},
        4=>{let mut bytes=[0u8;16];client.read_exact(&mut bytes)?;Ipv6Addr::from(bytes).to_string()},
        _=>{client.write_all(&[5,8,0,1,0,0,0,0,0,0])?;return Ok(());}
    };
    let mut port_bytes=[0u8;2];client.read_exact(&mut port_bytes)?;let port=u16::from_be_bytes(port_bytes);
    let mut remote=match upstream(source,&host,port){Ok(stream)=>stream,Err(_)=>{client.write_all(&[5,4,0,1,0,0,0,0,0,0])?;return Ok(());}};
    client.write_all(&[5,0,0,1,0,0,0,0,0,0])?;
    client.set_read_timeout(None)?;client.set_write_timeout(None)?;
    remote.set_read_timeout(None)?;remote.set_write_timeout(None)?;
    let mut from_client=client.try_clone()?;let mut to_remote=remote.try_clone()?;
    let send=thread::spawn(move||{let _=io::copy(&mut from_client,&mut to_remote);let _=to_remote.shutdown(std::net::Shutdown::Write);});
    let _=io::copy(&mut remote,&mut client);let _=client.shutdown(std::net::Shutdown::Write);let _=send.join();
    Ok(())
}
/// Listener lifetime is tied to the account helper process; no direct fallback.
pub fn start_proxy(source:Ipv6Addr,n:u8)->io::Result<()> {
    validate_local(source)?;
    let listener=TcpListener::bind(("127.0.0.1",proxy_port(n)))?;
    thread::spawn(move||for connection in listener.incoming(){if let Ok(stream)=connection {
        thread::spawn(move||{let _=handle(stream,source);});
    }});
    Ok(())
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn pool_accepts_only_distinct_global_ipv6() {
        assert_eq!(parse_pool("# comment\n2001:db8::1\n2001:db8::2 # note\n").unwrap().len(),2);
        for bad in ["127.0.0.1","::1","fe80::1","fc00::1","2001:db8::1\n2001:db8::1"] {assert!(parse_pool(bad).is_err());}
    }
    #[test] fn empty_pool_is_unconfigured() {
        assert!(parse_pool(TEMPLATE).unwrap().is_empty());
    }
    #[test] fn each_new_account_takes_first_unassigned_address() {
        let pool=parse_pool("2001:db8::1\n2001:db8::2\n2001:db8::3").unwrap();
        let mut used=HashSet::new();
        for expected in &pool {let selected=next_unused(&pool,&used).unwrap();assert_eq!(&selected,expected);used.insert(selected);}
        assert!(next_unused(&pool,&used).is_err());
    }
    #[test] fn socks5_tunnels_tcp_from_bound_ipv6_without_ipv4_fallback() {
        let echo=TcpListener::bind((Ipv6Addr::LOCALHOST,0)).unwrap();let port=echo.local_addr().unwrap().port();
        let echo_worker=thread::spawn(move||{let (mut stream,_)=echo.accept().unwrap();
            assert_eq!(stream.local_addr().unwrap().ip(),std::net::IpAddr::V6(Ipv6Addr::LOCALHOST));
            let mut data=[0u8;4];stream.read_exact(&mut data).unwrap();assert_eq!(&data,b"ping");stream.write_all(b"pong").unwrap();});
        let listener=TcpListener::bind(("127.0.0.1",0)).unwrap();let proxy_port=listener.local_addr().unwrap().port();
        let proxy=thread::spawn(move||{let (stream,_)=listener.accept().unwrap();handle(stream,Ipv6Addr::LOCALHOST).unwrap();});
        let mut client=TcpStream::connect(("127.0.0.1",proxy_port)).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        client.write_all(&[5,1,0]).unwrap();let mut greeting=[0;2];client.read_exact(&mut greeting).unwrap();assert_eq!(greeting,[5,0]);
        let mut request=vec![5,1,0,4];request.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());request.extend_from_slice(&port.to_be_bytes());client.write_all(&request).unwrap();
        let mut reply=[0;10];client.read_exact(&mut reply).unwrap();assert_eq!(reply[1],0);
        client.write_all(b"ping").unwrap();let mut answer=[0;4];client.read_exact(&mut answer).unwrap();assert_eq!(&answer,b"pong");
        drop(client);echo_worker.join().unwrap();proxy.join().unwrap();
        assert!(connect_from(Ipv6Addr::LOCALHOST,"127.0.0.1:443".parse().unwrap()).is_err());
    }
}

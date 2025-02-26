//! Check whether the operating system and/or the types in std supports
//! certain things or how they behave.
//! (tests that explore platform behavior are moved here after failing on some
//!  operating system.)

use std::fs::remove_file;
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::Path;
use std::io::{self, ErrorKind::*, Write};
use std::net::{TcpListener, TcpStream};
use std::mem::{self, ManuallyDrop};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

extern crate libc;

extern crate uds;
use uds::{UnixListenerExt, UnixStreamExt, UnixDatagramExt, UnixSocketAddr};
use uds::nonblocking::UnixSeqpacketConn;

fn max_path_len() -> usize {
    unsafe { mem::size_of_val(&mem::zeroed::<libc::sockaddr_un>().sun_path) }
}

fn std_bind_max_len_path() {
    print!("std_bind_max_len_path ");
    let max_len = max_path_len();
    let max_path = std::iter::repeat('B').take(max_len).collect::<String>();
    let _ = remove_file(&max_path);
    match UnixDatagram::bind(&max_path) {
        Ok(_) => {
            println!("yes");
            remove_file(&max_path).expect("delete socket file created by std");
        }
        Err(e) => println!("no ({})", e),
    }
}

fn max_path_addr(fill: u8) -> (libc::sockaddr_un, libc::socklen_t) {
    unsafe {
        let mut addr: libc::sockaddr_un = mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for dst in &mut addr.sun_path[..] {
            *dst = fill as libc::c_char;
        }
        let len = mem::size_of_val(&addr) as libc::socklen_t;
        (addr, len)
    }
}

fn std_get_local_max_len_path() {
    print!("std_get_local_max_len_path ");
    let max_len = UnixSocketAddr::max_path_len();
    let max_path = std::iter::repeat('s').take(max_len).collect::<String>();
    let max_addr = UnixSocketAddr::from_path(&max_path)
        .expect("create path address with max length");

    let _ = remove_file(&max_path);

    let listener = UnixListener::bind_unix_addr(&max_addr)
        .expect("create socket with max length path addr");
    let std_addr = listener.local_addr().expect("std get local max length path");
    match std_addr.as_pathname() {
        Some(std_path) if std_path == Path::new(&max_path) => println!("yes"),
        Some(std_path) => {
            let std_path = std_path.to_str().expect("convert Path that should be ASCII to str");
            println!(
                "buggy (returned path differs: {} vs {} ({} bytes vs {}))",
                std_path, max_path, std_path.len(), max_path.len()
            );
        },
        None => println!("no (as_pathname() returned None)"),
    }

    remove_file(&max_path).expect("delete socket file");
}

fn std_reply_max_len_path() {
    print!("std_reply_max_len ");
    let max_path = std::iter::repeat('S').take(max_path_len()).collect::<String>();
    let receiver_path = "max_path_receiver.socket";
    let receiver = UnixDatagram::bind(receiver_path).expect("create receiver socket");
    let sender = UnixDatagram::unbound().expect("create unix datagram socket");
    unsafe {
        let (addr, len) = max_path_addr(b'S');
        let addr_ptr = &addr as *const libc::sockaddr_un as *const libc::sockaddr;
        if libc::bind(sender.as_raw_fd(), addr_ptr, len) == -1 {
            println!("N/A (cannot bind from \"C\": {})", io::Error::last_os_error());
            return;
        }
    }
    sender.send_to(b"can you read me", receiver_path).expect("send from max addr");
    let (_bytes, addr) = receiver.recv_from(&mut[0; 20]).expect("receive from max addr");
    if let Some(path) = addr.as_pathname() {
        let path = path.to_str().unwrap();
        if path != max_path {
            println!(
                "buggy (received path differs: {} vs {} ({} bytes vs {}))",
                path, max_path, path.len(), max_path.len()
            );
        } else {
            println!("yes");
        }
    } else {
        println!("nonsensical received addr somehow isn't a path");
    }
    remove_file(receiver_path).expect("delete receiver socket file");
    remove_file(&max_path).expect("delete max len socket file");
}

fn longer_paths() {
    const MAX_EXTRA_TEST: usize = 100;
    #[repr(C)]
    struct LongAddr {
        sockaddr: libc::sockaddr_un,
        extra: [u8; MAX_EXTRA_TEST],
    }
    impl std::ops::Deref for LongAddr {
        type Target = [u8];
        fn deref(&self) -> &[u8] {
            let slice = unsafe {
                let included = std::mem::size_of_val(&self.sockaddr.sun_path);
                let extra = std::mem::size_of_val(&self.extra);
                let path_ptr = &self.sockaddr.sun_path[0] as *const _ as *const u8;
                assert_eq!(std::mem::size_of_val(&self.sockaddr)+extra, std::mem::size_of::<Self>());
                assert_eq!(
                    path_ptr as usize - self as *const Self as usize,
                    std::mem::size_of::<Self>() - included - extra
                );
                std::slice::from_raw_parts(path_ptr, included+extra)
            };
            &slice[..slice.iter().take_while(|&&b| b != b'\0' ).count()]
        }
    }
    fn new_longaddr(fill: u8,  extra_len: usize) -> (LongAddr, libc::socklen_t) {
        let mut addr = unsafe { std::mem::zeroed::<LongAddr>() };
        addr.sockaddr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        unsafe {
            let included = std::mem::size_of_val(&addr.sockaddr.sun_path);
            let len = included - 1 + extra_len;
            let extra = std::mem::size_of_val(&addr.extra);
            let combined = included + extra;
            if extra >= combined {
                panic!("{} bytes is too long for LongAddr", len);
            }
            let path_ptr = &mut addr.sockaddr.sun_path[0] as *mut _ as *mut u8;
            let path_offset = path_ptr as usize - &addr as *const LongAddr as usize;
            assert_eq!(
                path_offset,
                std::mem::size_of::<LongAddr>() - combined,
                "extended address is contigious"
            );
            let extended_path = std::slice::from_raw_parts_mut(path_ptr, combined);
            for i in 0..len {
                extended_path[i] = fill;
            }
            let addrlen = (path_offset + len + 1) as libc::socklen_t;
            (addr, addrlen)
        }
    }

    fn try_longer(fill: u8,  extra_len: usize) -> Result<bool, String> {
        let socket_a = UnixDatagram::unbound().unwrap();
        let (path_addr, addrlen) = new_longaddr(fill, extra_len);
        unsafe {
            let ret = libc::bind(
                socket_a.as_raw_fd(),
                &path_addr.sockaddr as *const _ as *const libc::sockaddr,
                addrlen
            );
            if ret == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINVAL) {
                    Ok(false)
                } else {
                    Err(format!("rejected with {} instead of EINVAL", error))
                }
            } else {
                match remove_file(std::str::from_utf8(&*path_addr).unwrap()) {
                    Err(ref err) if err.kind() == NotFound => {
                        Err(format!("bind() succeeded but path was not created"))
                    },
                    Ok(_) => Ok(true),
                    Err(err) => Err(format!("bind() succeeded but deleting file failed with {}", err)),
                }
            }
        }
    }

    print!("longer_paths ");
    match try_longer(b'P', 1) {
        Ok(true) => {}
        Ok(false) => {
            println!("no");
            return;
        }
        Err(description) => {
            println!("{}", description);
            return;
        }
    }
    for extra_len in 1..=MAX_EXTRA_TEST {
        match try_longer(b'a' + (extra_len as u8 % 10), extra_len) {
            Ok(true) => {}
            Ok(false) => {
                println!("{}", extra_len-1);
                return;
            }
            Err(description) => {
                println!("{} (at 100 {})", extra_len-1, description);
                return;
            }
        }
    }
    println!("100+");
}

fn std_includes_nuls_normal() {
    print!("std_includes_nuls_normal ");
    let regular_path = "normal.sock";
    let regular_addr = UnixSocketAddr::from_path(regular_path)
        .expect("create path address with max regular length");

    let _ = remove_file(regular_path);

    let listener = UnixListener::bind_unix_addr(&regular_addr)
        .expect("create socket with max regular path length");
    let std_addr = listener.local_addr().expect("std get local max length path");
    match std_addr.as_pathname() {
        Some(std_path) if std_path == Path::new(regular_path) => println!("no"),
        Some(std_path) => {
            let std_path = std_path.to_str().expect("convert Path that should be ASCII to str");
            if std_path.len() > regular_path.len()
            &&  &std_path[..regular_path.len()] == regular_path
            &&  std_path[regular_path.len()..].chars().all(|c| c == '\0' ) {
                println!("yes ({})", std_path.len() - regular_path.len());
            } else {
                println!(
                    "buggy (returned path differs in a different way: {:?} vs {:?} ({} bytes vs {}))",
                    std_path, regular_path, std_path.len(), regular_path.len()
                );
            }
        },
        None => println!("buggy (as_pathname() returned None)"),
    }

    remove_file(regular_path).expect("delete socket file");
}

fn std_includes_nuls_long() {
    print!("std_includes_nuls_long ");
    let max_regular_len = UnixSocketAddr::max_path_len()-1;
    let max_regular_path = std::iter::repeat('n').take(max_regular_len).collect::<String>();
    let max_regular_addr = UnixSocketAddr::from_path(&max_regular_path)
        .expect("create path address with max regular length");

    let _ = remove_file(&max_regular_path);

    let listener = UnixListener::bind_unix_addr(&max_regular_addr)
        .expect("create socket with max regular path length");
    let std_addr = listener.local_addr().expect("std get local max length path");
    match std_addr.as_pathname() {
        Some(std_path) if std_path == Path::new(&max_regular_path) => println!("no"),
        Some(std_path) => {
            let std_path = std_path.to_str().expect("convert Path that should be ASCII to str");
            if std_path.len() > max_regular_len
            &&  std_path[..max_regular_len] == max_regular_path
            &&  std_path[max_regular_len..].chars().all(|c| c == '\0' ) {
                println!("yes ({})", std_path.len() - max_regular_len);
            } else {
                println!(
                    "buggy (returned path differs in a different way: {:?} vs {:?} ({} bytes vs {}))",
                    std_path, max_regular_path, std_path.len(), max_regular_path.len()
                );
            }
        },
        None => println!("buggy (as_pathname() returned None)"),
    }

    remove_file(&max_regular_path).expect("delete socket file");
}

fn std_checks_family() {
    print!("std_checks_family ");
    let ip_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = ip_listener.local_addr().unwrap().port();
    let wrong = unsafe { ManuallyDrop::new(UnixListener::from_raw_fd(ip_listener.as_raw_fd())) };
    assert_eq!(wrong.local_addr().unwrap_err().kind(), InvalidInput);
    let _conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert_eq!(wrong.accept().unwrap_err().kind(), InvalidInput);
    println!("yes"); // FIXME if it ever doesn't
}

fn fd_must_remain_open_until_received() {
    print!("fd_must_remain_open_until_received ");
    let (a, b) = UnixDatagram::pair().expect("create datagram socket pair");
    b.set_nonblocking(true).expect("make receiving socket nonblocking");
    {
        let to_send = UnixDatagram::unbound().expect("create unbound datagram socket");
        assert!(to_send.local_addr().expect("get addr of unbound socket").is_unnamed());
        if let Err(e) = a.send_fds(b"non-empty", &[to_send.as_raw_fd()]) {
            println!("N/A sending fd failed with {}", e);
            return;
        }
    }
    let mut fd_buf = [-1; 1];
    match b.recv_fds(&mut[0; 10], &mut fd_buf[..]) {
        Ok((_, 1)) => {
            let received = unsafe { UnixDatagram::from_raw_fd(fd_buf[0]) };
            match received.local_addr() {
                Ok(_) => println!("no"),
                Err(e) => println!("yes (operation on fd failed with {})", e),
            }
        },
        Ok((_, 0)) => println!("yes (fd was not received)"),
        Ok((_, too_many)) => println!("N/A (received {} fds out of a single sent", too_many),
        Err(ref e) if e.kind() == WouldBlock => println!("yes (datagram got dropped"),
        Err(e) => println!("N/A (receive failed with unexpected reaseon {})", e),
    }
}

fn fd_might_not_be_cloned() {
    print!("fd_might_not_be_cloned ");
    let (a, b) = UnixDatagram::pair().expect("create datagram socket pair");
    b.set_nonblocking(true).expect("make receiving socket nonblocking");
    if let Err(e) = a.send_fds(b"non-empty", &[a.as_raw_fd()]) {
        println!("N/A sending fd failed with {}", e);
        return;
    }
    let mut fd_buf = [-1; 1];
    match b.recv_fds(&mut[0; 10], &mut fd_buf[..]) {
        Ok((_, 1)) if fd_buf[0] == a.as_raw_fd() => {
            drop(a);
            let received = unsafe { UnixDatagram::from_raw_fd(fd_buf[0]) };
            match received.local_addr() {
                Ok(_) => println!("maybe (fd {} still alive after close()d once)", fd_buf[0]),
                Err(_) => println!("yes (sent and got fd {})", fd_buf[0]),
            }
        }
        Ok((_, 1)) => {
            let received = unsafe { UnixDatagram::from_raw_fd(fd_buf[0]) };
            match received.local_addr() {
                Ok(_) => println!("no (sent fd {} got fd {})", a.as_raw_fd(), fd_buf[0]),
                Err(e) => println!("maybe (operation on different fd failed with {})", e),
            }
        }
        Ok((_, 0)) => println!("yes (fd was not received)"),
        Ok((_, too_many)) => println!("N/A (received {} fds out of a single sent", too_many),
        Err(ref e) if e.kind() == WouldBlock => println!("yes (datagram got dropped"),
        Err(e) => println!("N/A (receive failed with unexpected reaseon {})", e),
    }
}

fn fd_cloned_same_process() {
    print!("fd_cloned_same_process ");
    let (a, b) = UnixDatagram::pair().expect("create datagram socket pair");
    if let Err(e) = a.send_fds(b"intentionally left non-empty", &[a.as_raw_fd()]) {
        println!("N/A (send failed with {})", e);
    } else {
        let mut fd_buf = [-1; 2];
        match b.recv_fds(&mut[0; 32], &mut fd_buf) {
            Ok((_, 1)) if fd_buf[0] == a.as_raw_fd() => println!("no"),
            Ok((_, 1)) => {
                println!("yes");
                let _ = unsafe { UnixDatagram::from_raw_fd(fd_buf[0]) };
            },
            unexpected => println!("BUG recv_fds() returned {:?}", unexpected),
        }
    }
}

fn fd_cloned_on_send() {
    print!("fd_cloned_on_send ");
    let (a, b) = UnixDatagram::pair().expect("create datagram socket pair");
    if let Err(e) = a.send_fds(b"intentionally left non-empty", &[a.as_raw_fd()]) {
        println!("N/A (send failed with {})", e);
    } else {
        let a_fd = a.as_raw_fd();
        drop(a);
        let new_fd = UnixDatagram::unbound().expect("create unbound datagram socket");
        let mut fd_buf = [-1; 2];
        match b.recv_fds(&mut[0; 32], &mut fd_buf) {
            Ok((_, 1)) if fd_buf[0] == a_fd  &&  new_fd.as_raw_fd() == a_fd => {
                println!("N/A (original, now reused fd returned)");
            }
            Ok((_, 1)) => {
                let received = unsafe { UnixDatagram::from_raw_fd(fd_buf[0]) };
                match received.local_unix_addr() {
                    Ok(_) => println!("yes"),
                    Err(e) => println!("no ({})", e),
                }
            }
            unexpected => println!("BUG recv_fds() returned {:?}", unexpected)
        }
    }
}

fn stream_ancillary_payloads_not_merged() {
    print!("stream_ancillary_payloads_not_merged ");
    let (mut a, b) = UnixStream::pair().expect("create stream socket pair");
    b.set_nonblocking(true).expect("make receiving socket nonblocking");

    // send some then nothing
    if let Err(e) = a.send_fds(b"1", &[a.as_raw_fd()]) {
        println!("N/A ({})", e);
        return;
    }
    a.write(b"0").expect("write more bytes but no fds");
    let mut fd_buf = [-1; 6];
    match b.recv_fds(&mut[0u8; 20], &mut fd_buf) {
        Ok((1, 1)) if fd_buf[0] != -1  &&  fd_buf[1] == -1 => print!("yes "),
        Ok((bytes, fds)) => print!("no ({} bytes, {} fds) ", bytes, fds),
        Err(e) => print!("no ({})", e),
    }
    if fd_buf[0] != a.as_raw_fd() {
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[0]) };
    }

    let mut fd_buf = [-1; 6];
    match b.recv_fds(&mut[0u8; 20], &mut fd_buf) {
        Ok((1, 0)) if fd_buf[0] == -1 => print!("yes "),
        Ok((bytes, fds)) => print!("no ({} bytes, {} fds) ", bytes, fds),
        Err(e) => print!("no ({}) ", e),
    }

    // send twice
    a.send_fds(b"2", &[a.as_raw_fd(), a.as_raw_fd()]).expect("send two fds");
    a.send_fds(b"3", &[b.as_raw_fd(), b.as_raw_fd(), b.as_raw_fd()])
        .expect("write three more fds");
    let mut fd_buf = [-1; 6];
    match b.recv_fds(&mut[0u8; 3], &mut fd_buf) {
        Ok((1, 2)) if fd_buf[1] != -1  &&  fd_buf[2] == -1 => print!("yes "),
        Ok((bytes, fds)) => print!("no ({} bytes, {} fds) ", bytes, fds),
        Err(e) => print!("no ({})", e),
    }
    if fd_buf[0] != a.as_raw_fd()  &&  fd_buf[0] != b.as_raw_fd() {
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[0]) };
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[1]) };
    }

    let mut fd_buf = [-1; 6];
    match b.recv_fds(&mut[0u8; 3], &mut fd_buf) {
        Ok((1, 3)) if fd_buf[2] != -1  &&  fd_buf[3] == -1 => println!("yes"),
        Ok((bytes, fds)) => println!("no ({} bytes, {} fds)", bytes, fds),
        Err(e) => println!("no ({})", e),
    }
    if fd_buf[0] != a.as_raw_fd()  &&  fd_buf[0] != b.as_raw_fd() {
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[0]) };
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[1]) };
        let _ = unsafe { UnixStream::from_raw_fd(fd_buf[2]) };
    }
}

fn seqpacket_recv_empty() {
    print!("seqpacket_recv_empty ");
    let (a, b) = match UnixSeqpacketConn::pair() {
        Ok(pair) => pair,
        Err(e) => {
            println!("N/A ({})", e); // seqpacket not supported
            return;
        }
    };

    match b.recv(&mut[]) {
        Ok(0) => {
            println!("always"); // receive always succeeds
            return;
        },
        Err(ref e) if e.kind() == WouldBlock => {},
        unexpected => {
            println!("strange (recv() with empty buffer returned {:?})", unexpected);
            return;
        }
    }
    a.send(&[]).expect("send empty packet without ancillary");
    match b.recv(&mut[0; 8]) {
        Ok(0) => println!("yes"), // fully supported
        Err(ref e) if e.kind() == WouldBlock => println!("no"), // empty send ignored
        unexpected => println!("strange (recv() returned {:?})", unexpected),
    }
    // TODO test receiving with ancillary
}

fn accept_timeout() {
    print!("accept_timeout ");

    let name = "accept_timeout.sock";
    let _ = remove_file(name);
    let listener = UnixListener::bind(name).expect("create stream listener");

    unsafe {
        let timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: 10,
        };
        let status = libc::setsockopt(
            listener.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const _ as *const _,
            mem::size_of_val(&timeout) as libc::socklen_t,
        );
        if status == -1 {
            println!("no (setting timeout failed: {})", io::Error::last_os_error());
            return;
        }
    }

    let (tx, rx) = mpsc::channel();
    thread::spawn(move|| tx.send(listener.accept()) );
    match rx.recv_timeout(Duration::new(0, 10_000_000)) {
        Ok(Err(e)) if e.kind() == WouldBlock => println!("yes"),
        Ok(Err(e)) => println!("buggy (accept() failed with unexpected error {})", e),
        Ok(Ok(_)) => println!("buggy (accept() unexpectedly succeeded)"),
        Err(mpsc::RecvTimeoutError::Timeout) => println!("no"),
        Err(mpsc::RecvTimeoutError::Disconnected) => println!("buggy (thread exited without sending!)"),
    }
    // don't try to join a hung thread

    remove_file(name).expect("delete socket file");
}

fn print_credentials() {
    print!("peer credentials ");
    let _ = remove_file("conncreds.socket");
    let _listener = UnixListener::bind("conncreds.socket")
        .expect("create conncreds.socket and listen to it");
    let client = UnixStream::connect("conncreds.socket")
        .expect("connect to conncreds.socket");
    remove_file("conncreds.socket").expect("delete created socket file");
    match client.initial_peer_credentials() {
        Ok(creds) => println!("yes ({:?})", creds),
        Err(e) => println!("no ({})", e),
    }
    drop((client, _listener));

    print!("pair credentials ");
    let (a, _b) = UnixStream::pair().expect("create stream socket pair");
    match a.initial_peer_credentials() {
        Ok(creds) => println!("yes ({:?})", creds),
        Err(e) => println!("no ({})", e), // fails on DragonFly BSD and NetBSD
    }

    print!("SELinux_context ");
    let mut buf = [0u8; 1024];
    match a.initial_peer_selinux_context(&mut buf) {
        Ok(len) => println!("yes ({:?} ({} bytes))", String::from_utf8_lossy(&buf[..len]), len),
        Err(e) => println!("no ({})", e),
    }
}

fn main() {
    println!("OS {}", std::env::consts::OS);
    std_bind_max_len_path();
    std_get_local_max_len_path();
    std_reply_max_len_path();
    longer_paths();
    std_includes_nuls_normal();
    std_includes_nuls_long();
    std_checks_family();
    fd_must_remain_open_until_received();
    fd_might_not_be_cloned();
    fd_cloned_same_process();
    fd_cloned_on_send();
    stream_ancillary_payloads_not_merged();
    seqpacket_recv_empty();
    accept_timeout();
    print_credentials();
}

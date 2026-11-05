use std::{
    io::Read,
    net::{TcpListener, TcpStream},
    process::{Child, Output},
    thread,
    time::{Duration, Instant},
};

pub fn wait_output(mut child: Child) -> Output {
    let mut stdout = child.stdout.take().expect("Capture child stdout");
    let mut stderr = child.stderr.take().expect("Capture child stderr");
    let out = thread::spawn(move || {
        let mut data = vec![];
        stdout.read_to_end(&mut data).unwrap();
        data
    });
    let err = thread::spawn(move || {
        let mut data = vec![];
        stderr.read_to_end(&mut data).unwrap();
        data
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        thread::sleep(Duration::from_millis(5));
    };
    let output = Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    };
    assert!(!timed_out, "CLI child exceeded its 30-second test deadline");
    output
}

// Used by HTTPS fixtures; CLI-only tests do not bind a listener.
#[allow(dead_code)]
pub fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("Test server failed to accept a request before its deadline: {e}"),
        }
    }
}

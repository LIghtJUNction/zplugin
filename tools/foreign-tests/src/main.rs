use foreign_interface::{Event, ForeignApi, Mode, Point};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use zplugin::{Error, Plugin, UnloadPolicy};

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments
        .next()
        .expect("usage: zplugin-foreign-tests --header PATH | PLUGIN...");
    if first == "--header" {
        let path = arguments.next().expect("header output path");
        assert!(arguments.next().is_none(), "unexpected argument");
        let mut header = String::new();
        ForeignApi::write_c_header(&mut header).expect("header formatting");
        if let Some(parent) = Path::new(&path)
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).expect("header output directory");
        }
        std::fs::write(path, header).expect("write generated header");
        return;
    }
    for path in std::iter::once(first).chain(arguments) {
        verify(Path::new(&path));
    }
}

fn verify(path: &Path) {
    // SAFETY: these are locally built test plugins implementing the documented
    // ABI and cooperative shutdown. This function does not accept untrusted code.
    let library = Rc::new(unsafe { Plugin::load_path(path, UnloadPolicy::Cooperative) }.unwrap());
    let mut plugin =
        ForeignApi::wrap(Plugin::create_owned(&library, ForeignApi::SPEC).unwrap()).unwrap();
    assert_eq!(plugin.calculate(Point { x: 6, y: 7 }, Mode::Sum), Ok(13));
    assert_eq!(
        plugin.calculate(Point { x: 6, y: 7 }, Mode::Product),
        Ok(42)
    );
    for value in [
        Event::Empty,
        Event::Number { value: u64::MAX },
        Event::Position {
            point: Point { x: -7, y: 99 },
        },
    ] {
        assert_eq!(plugin.echo(value), Ok(value));
    }
    assert_eq!(plugin.transform(&[], &mut []), Ok(0));
    let mut output = [0_u8; 8];
    assert_eq!(plugin.transform(b"native", &mut output), Ok(6));
    assert_eq!(&output[..6], b"NATIVE");
    let mut short = [0xa5_u8; 1];
    assert_eq!(
        plugin.transform(b"long", &mut short),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(short, [0xa5]);
    let bytes = plugin.owned(b"ownership").unwrap();
    assert_eq!(&*bytes, b"ownership");
    assert!(plugin.owned(&[]).unwrap().is_empty());
    let mut resource = plugin.resource().unwrap();
    resource.check("foreign.reverse", 1).unwrap();
    assert!(resource.check("foreign.reverse", 2).is_err());
    assert!(resource.check("foreign.other", 1).is_err());
    assert_eq!(resource.call(1, b"abc", &mut output), Ok(3));
    assert_eq!(&output[..3], b"cba");
    assert_eq!(
        resource.call(2, b"abc", &mut output),
        Err(Error::Unsupported)
    );

    let callback = zplugin::Callback::new(|input, output| {
        if output.len() < input.len() {
            return Err(Error::BufferTooSmall);
        }
        for (source, destination) in input.iter().zip(output.iter_mut()) {
            *destination = source ^ 0x20;
        }
        Ok(input.len())
    });
    assert_eq!(
        plugin.callback(callback.as_ref(), b"Call", &mut output),
        Ok(4)
    );
    assert_eq!(&output[..4], b"cALL");
    assert_eq!(
        plugin.callback(callback.as_ref(), b"large", &mut short),
        Err(Error::BufferTooSmall)
    );
    let panicking = zplugin::Callback::new(|_, _| panic!("expected reverse callback panic"));
    assert_eq!(
        plugin.callback(panicking.as_ref(), &[], &mut []),
        Err(Error::Panicked)
    );
    assert_eq!(
        plugin.callback(panicking.as_ref(), &[], &mut []),
        Err(Error::Poisoned)
    );

    let wakes = Arc::new(Counter(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    let mut task = plugin.start(41).unwrap();
    assert_eq!(Pin::new(&mut task).poll(&mut cx), Poll::Pending);
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
    let mut cancelled = plugin.start(7).unwrap();
    assert_eq!(Pin::new(&mut cancelled).poll(&mut cx), Poll::Pending);
    cancelled.cancel().unwrap();
    cancelled.cancel().unwrap();
    assert_eq!(
        Pin::new(&mut cancelled).poll(&mut cx),
        Poll::Ready(Err(Error::Cancelled))
    );
    drop(cancelled);
    drop(plugin);
    let library = Rc::try_unwrap(library)
        .err()
        .expect("owned results retain the library");
    // These values each keep code and originating allocator alive independently
    // of the object that returned them.
    assert_eq!(&*bytes, b"ownership");
    assert_eq!(resource.call(1, b"late", &mut output), Ok(4));
    assert_eq!(&output[..4], b"etal");
    assert_eq!(Pin::new(&mut task).poll(&mut cx), Poll::Ready(Ok(42)));
    task.cancel().unwrap();
    drop(task);
    drop(resource);
    drop(bytes);
    if path.file_stem().is_some_and(|name| name == "native_cpp") {
        let mut exceptions = ForeignApi::create(&library).unwrap();
        assert_eq!(
            exceptions.calculate(Point { x: -99, y: 0 }, Mode::Sum),
            Err(Error::Panicked)
        );
        assert_eq!(
            exceptions.calculate(Point { x: 1, y: 2 }, Mode::Sum),
            Err(Error::Poisoned)
        );
        assert_eq!(exceptions.transform(&[], &mut []), Err(Error::Poisoned));
    }
    Rc::try_unwrap(library)
        .ok()
        .unwrap()
        .try_close()
        .ok()
        .unwrap();
    println!(
        "PASS foreign plugin {}: typed values, slices, owned bytes/resources, callback, task wake/cancel, cooperative release",
        path.display()
    );
}

struct Counter(AtomicUsize);
impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

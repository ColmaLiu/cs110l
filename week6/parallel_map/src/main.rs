use crossbeam_channel;
use std::{thread, time};

fn parallel_map<T, U, F>(mut input_vec: Vec<T>, num_threads: usize, f: F) -> Vec<U>
where
    F: FnOnce(T) -> U + Send + Copy + 'static,
    T: Send + 'static,
    U: Send + 'static + Default,
{
    let mut output_vec: Vec<U> = Vec::with_capacity(input_vec.len());
    // TODO: implement parallel map!
    output_vec.resize_with(input_vec.len(), Default::default);
    let mut threads = Vec::new();
    let (sender1, receiver1) = crossbeam_channel::unbounded();
    let (sender2, receiver2) = crossbeam_channel::unbounded();
    for _ in 0..num_threads {
        let receiver1 = receiver1.clone();
        let sender2 = sender2.clone();
        threads.push(thread::spawn(move || {
            while let Ok((index, val)) = receiver1.recv() {
                sender2.send((index, f(val))).unwrap();
            }
        }));
    }
    for (index, val) in input_vec.into_iter().enumerate() {
        sender1.send((index, val)).unwrap();
    }
    drop(sender1);
    drop(sender2);
    while let Ok((index, val)) = receiver2.recv() {
        output_vec[index] = val;
    }
    for thread in threads {
        thread.join().unwrap();
    }
    output_vec
}

fn main() {
    let v = vec![6, 7, 8, 9, 10, 1, 2, 3, 4, 5, 12, 18, 11, 5, 20];
    let squares = parallel_map(v, 10, |num| {
        println!("{} squared is {}", num, num * num);
        thread::sleep(time::Duration::from_millis(500));
        num * num
    });
    println!("squares: {:?}", squares);
}

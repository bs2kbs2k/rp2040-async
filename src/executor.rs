extern crate alloc;
use alloc::{boxed::Box, vec::Vec};
use core::{
    borrow::BorrowMut,
    future::Future,
    mem::forget,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use crate::sync::{Arc, Mutex};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>>;
type ArcMutexFut = Arc<Mutex<BoxFuture<()>, 5>, 6>;

static TASK_QUEUE: Mutex<Vec<ArcMutexFut>, 0> = Mutex::new(Vec::new());

// Poll all tasks that can be polled.
pub fn tick() {
    let mut queue = TASK_QUEUE.lock();
    while let Some(task) = queue.pop() {
        let fut = task.borrow_mut().lock().as_mut();
        let waker = unsafe { Waker::from_raw(construct_waker(task.clone())) };
        fut.poll(&mut Context::from_waker(&waker));
    }
}

fn construct_waker(future: ArcMutexFut) -> RawWaker {
    let vtable = unsafe {
        RawWakerVTable::new(
            |data| unsafe {
                let data: ArcMutexFut = Arc::from_raw(data);
                let ret = construct_waker(data.clone());
                forget(data); // Do NOT drop the ArcMutexFut here: this is still retained by the waker.
                ret
            },
            |data| unsafe {
                let data: ArcMutexFut = Arc::from_raw(data);
                TASK_QUEUE.lock().push(data);
                drop(data); // Drop the ArcMutexFut here: it is no longer retained by the waker.
            },
            |data| unsafe {
                let data: ArcMutexFut = Arc::from_raw(data);
                TASK_QUEUE.lock().push(data.clone());
                forget(data); // Do NOT drop the ArcMutexFut here: this is still retained by the waker.
            },
            |data| unsafe {
                let data: ArcMutexFut = Arc::from_raw(data);
                drop(data); // We're dropping the ArcMutexFut to clean up.
            },
        )
    };
    let waker = RawWaker::new(future.clone().to_raw(), &vtable);
    waker
}

fn spawn_inner(task: impl Future<Output = ()> + Send + Sync + 'static) {
    let mut queue = TASK_QUEUE.lock();
    queue.push(Arc::new(Mutex::new(Box::pin(task))));
}

// Spawn a task. The task will be ran to completion.
// The returned future will complete when the task is completed.
pub fn spawn<T>(task: impl Future<Output = T> + Send + Sync + 'static) -> impl Future<Output = T>
where
    T: Send + Sync,
{
    TaskHandle::new(task)
}

struct TaskHandle<T> {
    waker: Arc<Mutex<Option<Waker>, 1>, 2>,
    return_value: Arc<Mutex<Option<T>, 3>, 4>,
}

impl<T> TaskHandle<T>
where
    T: Send + Sync,
{
    fn new(task: impl Future<Output = T> + Send + Sync + 'static) -> Self {
        let waker = Arc::new(Mutex::new(None));
        let return_value = Arc::new(Mutex::new(None));
        let ret = TaskHandle {
            waker: waker.clone(),
            return_value: return_value.clone(),
        };
        crate::executor::spawn_inner(async move {
            let ret = task.await;
            let mut return_value = return_value.lock();
            *return_value = Some(ret);
            let mut waker = waker.lock();
            if let Some(waker) = waker.take() {
                waker.wake();
            }
        });
        TaskHandle {
            waker,
            return_value,
        }
    }
}

impl<T> Future for TaskHandle<T>
where
    T: Send + Sync,
{
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<T> {
        let mut return_value = self.return_value.lock();
        if let Some(return_value) = return_value.take() {
            Poll::Ready(return_value)
        } else {
            let mut waker = self.waker.lock();
            *waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

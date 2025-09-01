//! Utilities for compute-heavy tasks which need to be run in parallel.

use std::ops::Deref;
use std::sync::Mutex;

/// A factory which produces a resource for use with [`ResourcePool`].
pub trait Resource {
    /// The type of the resource to be produced.
    type Output;

    /// An error type.
    type Error;

    /// Constructor for the resource.
    fn try_create(&self) -> Result<Self::Output, Self::Error>;
}

/// An unbounded pool of on-demand generated resources. This is useful when
/// distributing work across threads which needs access to an expensive-to-build
/// context.
///
/// A new resource is created when there isn't one available in the pool. When
/// it's dropped, it's returned to the pool. Old resources remain in the pool
/// until the pool itself is dropped.
///
/// ```
/// # use std::cell::RefCell;
/// # use branchless::core::task::{Resource, ResourceHandle, ResourcePool};
/// struct MyResource {
///     num_instantiations: RefCell<usize>,
/// }
///
/// impl Resource for MyResource {
///     type Output = String;
///     type Error = std::convert::Infallible;
///     fn try_create(&self) -> Result<Self::Output, Self::Error> {
///         let mut r = self.num_instantiations.borrow_mut();
///         *r += 1;
///         Ok(format!("This is resource #{}", *r))
///     }
/// }
///
/// # fn main() {
/// let resource = MyResource { num_instantiations: Default::default() };
/// let pool = ResourcePool::new(resource);
///
/// // Any number of the resource can be created.
/// let r1: ResourceHandle<'_, MyResource> = pool.try_create().unwrap();
/// assert_eq!(&*r1, "This is resource #1");
/// let r2 = pool.try_create().unwrap();
/// assert_eq!(&*r2, "This is resource #2");
/// drop(r2);
/// drop(r1);
///
/// // After releasing a resource, an attempt to get a resource returns an
/// // existing one from the pool.
/// let r1_again = pool.try_create().unwrap();
/// assert_eq!(&*r1_again, "This is resource #1");
/// # }
/// ```
pub struct ResourcePool<R: Resource> {
    factory: R,
    resources: Mutex<Vec<R::Output>>,
}

impl<R: Resource> std::fmt::Debug for ResourcePool<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourcePool")
            .field("factory", &"<not shown>")
            .field(
                "resources.len()",
                &match self.resources.try_lock() {
                    Ok(resources) => resources.len().to_string(),
                    Err(_) => "<could not determine>".to_string(),
                },
            )
            .finish()
    }
}

/// A handle to an instance of a resource created by [`Resource::try_create`].
/// When this value is dropped, the underlying resource returns to the owning
/// [`ResourcePool`].
pub struct ResourceHandle<'pool, R: Resource> {
    parent: &'pool ResourcePool<R>,
    inner: Option<R::Output>,
}

impl<R: Resource> Drop for ResourceHandle<'_, R> {
    fn drop(&mut self) {
        let mut resources = self
            .parent
            .resources
            .lock()
            .expect("Poisoned mutex for ResourceHandle");
        resources.push(self.inner.take().unwrap());
    }
}

impl<R: Resource> Deref for ResourceHandle<'_, R> {
    type Target = R::Output;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}

impl<R: Resource> ResourcePool<R> {
    /// Constructor.
    pub fn new(factory: R) -> Self {
        ResourcePool {
            factory,
            resources: Default::default(),
        }
    }

    /// If there are any resources available in the pool, return an arbitrary
    /// one. Otherwise, invoke the constructor function of the associated
    /// [`Resource`] and return a [`ResourceHandle`] for it.
    pub fn try_create(&self) -> Result<ResourceHandle<'_, R>, R::Error> {
        let resource = {
            let mut resources = self
                .resources
                .lock()
                .expect("Poisoned mutex for ResourcePool");
            let resource = resources.pop();
            match resource {
                Some(resource) => resource,
                None => self.factory.try_create()?,
            }
        };
        Ok(ResourceHandle {
            parent: self,
            inner: Some(resource),
        })
    }
}

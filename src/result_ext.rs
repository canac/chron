/// Extension trait for `Result` to provide `filter_err`
pub trait ResultExt<T, E> {
    fn filter_err<F>(self, predicate: F) -> Result<T, E>
    where
        F: FnOnce(&E) -> bool;
}

impl<T: Default, E> ResultExt<T, E> for Result<T, E> {
    /// Returns `Ok` if the contained `Err` value matches a predicate, leaving an `Err` value untouched.
    fn filter_err<F>(self, predicate: F) -> Self
    where
        F: FnOnce(&E) -> bool,
    {
        let Err(err) = self else { return self };
        if predicate(&err) {
            Ok(Default::default())
        } else {
            Err(err)
        }
    }
}

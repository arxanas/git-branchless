pub(crate) trait UsizeExt {
    fn unwrap_isize(self) -> isize;
}

impl UsizeExt for usize {
    fn unwrap_isize(self) -> isize {
        isize::try_from(self).unwrap()
    }
}

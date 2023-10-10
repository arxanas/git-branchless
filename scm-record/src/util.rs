pub(crate) trait UsizeExt {
    fn unwrap_isize(self) -> isize;
    fn clamp_into_u16(self) -> u16;
}

impl UsizeExt for usize {
    fn unwrap_isize(self) -> isize {
        isize::try_from(self).unwrap()
    }

    fn clamp_into_u16(self) -> u16 {
        if self > u16::MAX.try_into().unwrap() {
            u16::MAX
        } else {
            self.try_into().unwrap()
        }
    }
}

pub(crate) trait IsizeExt {
    fn unwrap_usize(self) -> usize;
    fn clamp_into_u16(self) -> u16;
    fn clamp_into_usize(self) -> usize;
}

impl IsizeExt for isize {
    fn unwrap_usize(self) -> usize {
        usize::try_from(self).unwrap()
    }

    fn clamp_into_u16(self) -> u16 {
        if self < 0 {
            0
        } else {
            self.try_into().unwrap_or(u16::MAX)
        }
    }

    fn clamp_into_usize(self) -> usize {
        if self < 0 {
            0
        } else {
            self.try_into().unwrap_or(usize::MAX)
        }
    }
}

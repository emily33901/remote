use std::mem::MaybeUninit;

use windows::Win32::Media::MediaFoundation::IMFMediaBuffer;

use eyre::Result;

pub(crate) fn with_locked_media_buffer<F: FnOnce(&mut [u8], &mut usize) -> Result<()>>(
    buffer: &IMFMediaBuffer,
    f: F,
) -> Result<()> {
    unsafe {
        let mut begin: MaybeUninit<*mut u8> = MaybeUninit::uninit();
        let mut len = buffer.GetCurrentLength()?;
        let mut max_len = buffer.GetMaxLength()?;

        buffer.Lock(
            &mut begin as *mut _ as *mut *mut u8,
            Some(&mut max_len),
            Some(&mut len),
        )?;

        scopeguard::defer! { buffer.Unlock().unwrap() };

        let mut len = len as usize;
        let max_len = max_len as usize;

        let mut s = std::slice::from_raw_parts_mut(begin.assume_init(), max_len);
        f(&mut s, &mut len)?;

        buffer.SetCurrentLength(len as u32)?;
    }

    Ok(())
}

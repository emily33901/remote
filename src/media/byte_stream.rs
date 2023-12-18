use windows::core::implement;
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncResult, IMFByteStream, IMFByteStream_Impl, MFBYTESTREAM_SEEK_ORIGIN,
};

#[implement(IMFByteStream)]
pub(crate) struct ByteStream {}

impl IMFByteStream_Impl for ByteStream {
    fn GetCapabilities(&self) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn GetLength(&self) -> ::windows::core::Result<u64> {
        todo!()
    }

    fn SetLength(&self, qwlength: u64) -> ::windows::core::Result<()> {
        todo!()
    }

    fn GetCurrentPosition(&self) -> ::windows::core::Result<u64> {
        todo!()
    }

    fn SetCurrentPosition(&self, qwposition: u64) -> ::windows::core::Result<()> {
        todo!()
    }

    fn IsEndOfStream(&self) -> ::windows::core::Result<windows::Win32::Foundation::BOOL> {
        todo!()
    }

    fn Read(&self, pb: *mut u8, cb: u32, pcbread: *mut u32) -> ::windows::core::Result<()> {
        todo!()
    }

    fn BeginRead(
        &self,
        pb: *mut u8,
        cb: u32,
        pcallback: ::core::option::Option<&IMFAsyncCallback>,
        punkstate: ::core::option::Option<&::windows::core::IUnknown>,
    ) -> ::windows::core::Result<()> {
        todo!()
    }

    fn EndRead(
        &self,
        presult: ::core::option::Option<&IMFAsyncResult>,
    ) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn Write(&self, pb: *const u8, cb: u32) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn BeginWrite(
        &self,
        pb: *const u8,
        cb: u32,
        pcallback: ::core::option::Option<&IMFAsyncCallback>,
        punkstate: ::core::option::Option<&::windows::core::IUnknown>,
    ) -> ::windows::core::Result<()> {
        todo!()
    }

    fn EndWrite(
        &self,
        presult: ::core::option::Option<&IMFAsyncResult>,
    ) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn Seek(
        &self,
        seekorigin: MFBYTESTREAM_SEEK_ORIGIN,
        llseekoffset: i64,
        dwseekflags: u32,
    ) -> ::windows::core::Result<u64> {
        todo!()
    }

    fn Flush(&self) -> ::windows::core::Result<()> {
        todo!()
    }

    fn Close(&self) -> ::windows::core::Result<()> {
        todo!()
    }
}

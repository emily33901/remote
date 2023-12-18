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

    fn SetLength(&self, _qwlength: u64) -> ::windows::core::Result<()> {
        todo!()
    }

    fn GetCurrentPosition(&self) -> ::windows::core::Result<u64> {
        todo!()
    }

    fn SetCurrentPosition(&self, _qwposition: u64) -> ::windows::core::Result<()> {
        todo!()
    }

    fn IsEndOfStream(&self) -> ::windows::core::Result<windows::Win32::Foundation::BOOL> {
        todo!()
    }

    fn Read(&self, _pb: *mut u8, _cb: u32, _pcbread: *mut u32) -> ::windows::core::Result<()> {
        todo!()
    }

    fn BeginRead(
        &self,
        _pb: *mut u8,
        _cb: u32,
        _pcallback: ::core::option::Option<&IMFAsyncCallback>,
        _punkstate: ::core::option::Option<&::windows::core::IUnknown>,
    ) -> ::windows::core::Result<()> {
        todo!()
    }

    fn EndRead(
        &self,
        _presult: ::core::option::Option<&IMFAsyncResult>,
    ) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn Write(&self, _pb: *const u8, _cb: u32) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn BeginWrite(
        &self,
        _pb: *const u8,
        _cb: u32,
        _pcallback: ::core::option::Option<&IMFAsyncCallback>,
        _punkstate: ::core::option::Option<&::windows::core::IUnknown>,
    ) -> ::windows::core::Result<()> {
        todo!()
    }

    fn EndWrite(
        &self,
        _presult: ::core::option::Option<&IMFAsyncResult>,
    ) -> ::windows::core::Result<u32> {
        todo!()
    }

    fn Seek(
        &self,
        _seekorigin: MFBYTESTREAM_SEEK_ORIGIN,
        _llseekoffset: i64,
        _dwseekflags: u32,
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

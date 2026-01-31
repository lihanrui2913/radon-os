use alloc::{format, string::String, sync::Arc, vec, vec::Vec};
use gpt_disk_io::{
    gpt_disk_types::{BlockSize, GptPartitionEntryArrayLayout, GptPartitionEntrySize},
    BlockIo, DiskError,
};
use libdriver::{
    protocol::IoRequest,
    server::{ConnectionContext, RequestContext},
    DriverOp, Request, RequestHandler, Response,
};
use radon_kernel::Result;

pub const BLOCK_SUCCESS: i32 = 0;
pub const BLOCK_ERR_IO: i32 = 1;

pub trait BlockDevice {
    fn read_block(&self, start_byte: u64, buf: &mut [u8]) -> Result<()>;
    fn write_block(&self, start_byte: u64, buf: &[u8]) -> Result<()>;
    fn size(&self) -> usize;
}

#[derive(Clone)]
pub struct PartitionDevice {
    inner: Arc<dyn BlockDevice>,
    offset: u64,
    size: usize,
}

unsafe impl Send for PartitionDevice {}
unsafe impl Sync for PartitionDevice {}

impl RequestHandler for PartitionDevice {
    fn handle(&self, request: &Request, _ctx: &RequestContext) -> Response {
        match DriverOp::from(request.header.op) {
            DriverOp::Read => {
                let io_request =
                    unsafe { (request.data.as_ptr() as *const IoRequest).as_ref() }.unwrap();
                let mut buf = Vec::with_capacity(io_request.length as usize);
                if let Err(_) = self.read_block(io_request.offset, &mut buf) {
                    Response::error(request.header.request_id, BLOCK_ERR_IO)
                } else {
                    Response::success(request.header.request_id).with_data(buf)
                }
            }
            DriverOp::Write => {
                let io_request =
                    unsafe { (request.data.as_ptr() as *const IoRequest).as_ref() }.unwrap();
                let buf = unsafe {
                    core::slice::from_raw_parts(
                        (request.data.as_ptr() as *const IoRequest).add(1) as *const u8,
                        io_request.length as usize,
                    )
                };
                if let Err(_) = self.write_block(io_request.offset, buf) {
                    Response::error(request.header.request_id, BLOCK_ERR_IO)
                } else {
                    Response::success(request.header.request_id)
                        .with_data((io_request.length).to_le_bytes().to_vec())
                }
            }
            // TODO: GetBuffer & ReleaseBuffer
            _ => Response::error(request.header.request_id, 1),
        }
    }

    fn on_connect(&self, _ctx: &ConnectionContext) -> libdriver::Result<()> {
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ConnectionContext) {}
}

impl BlockDevice for PartitionDevice {
    fn read_block(&self, start_byte: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_block(start_byte + self.offset, buf)
    }

    fn write_block(&self, start_byte: u64, buf: &[u8]) -> Result<()> {
        self.inner.write_block(start_byte + self.offset, buf)
    }

    fn size(&self) -> usize {
        self.size
    }
}

pub struct TmpBlock(Arc<dyn BlockDevice>);

impl BlockIo for TmpBlock {
    type Error = radon_kernel::Error;

    fn block_size(&self) -> BlockSize {
        BlockSize::from_usize(512).unwrap()
    }

    fn read_blocks(
        &mut self,
        start_lba: gpt_disk_io::gpt_disk_types::Lba,
        dst: &mut [u8],
    ) -> core::result::Result<(), Self::Error> {
        self.0.read_block(start_lba.to_u64() * 512, dst)
    }

    fn write_blocks(
        &mut self,
        start_lba: gpt_disk_io::gpt_disk_types::Lba,
        src: &[u8],
    ) -> core::result::Result<(), Self::Error> {
        self.0.write_block(start_lba.to_u64() * 512, src)
    }

    fn flush(&mut self) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn num_blocks(&mut self) -> core::result::Result<u64, Self::Error> {
        Ok(self.0.size() as u64 / 512)
    }
}

pub fn probe_parititons(
    prefix: &str,
    block_dev: Arc<dyn BlockDevice>,
    f: fn(String, PartitionDevice),
) -> Result<(), DiskError<usize>> {
    let mut disk = gpt_disk_io::Disk::new(TmpBlock(block_dev.clone())).unwrap();

    let mut buf = vec![0u8; 512 * 8 * 100];
    if let Ok(header) = disk.read_primary_gpt_header(&mut buf) {
        if let Ok(part_iter) = disk.gpt_partition_entry_array_iter(
            GptPartitionEntryArrayLayout {
                start_lba: header.partition_entry_lba.into(),
                entry_size: GptPartitionEntrySize::new(header.size_of_partition_entry.to_u32())
                    .ok()
                    .ok_or(DiskError::Io(0))?,
                num_entries: header.number_of_partition_entries.to_u32(),
            },
            &mut buf,
        ) {
            for (id, part) in part_iter.enumerate() {
                if let Ok(part) = part {
                    if !part.is_used() {
                        break;
                    }
                    let partdev = PartitionDevice {
                        inner: block_dev.clone(),
                        offset: part.starting_lba.to_u64() * 512,
                        size: (part.ending_lba.to_u64() - part.starting_lba.to_u64()) as usize
                            * 512,
                    };
                    f(format!("{}part{}", prefix, id), partdev);
                }
            }
            return Ok(());
        }
    }

    f(
        format!("{}part0", prefix),
        PartitionDevice {
            inner: block_dev.clone(),
            offset: 0,
            size: block_dev.size(),
        },
    );

    Ok(())
}

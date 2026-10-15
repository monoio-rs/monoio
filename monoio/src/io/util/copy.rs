#![allow(unused)]

use std::io;

use crate::io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt};
#[cfg(unix)]
use crate::net::unix::{new_pipe, Pipe};

const BUF_SIZE: usize = 64 * 1024;

/// Copy data from reader to writer.
pub async fn copy<'a, R, W>(reader: &'a mut R, writer: &'a mut W) -> io::Result<u64>
where
    R: AsyncReadRent + ?Sized,
    W: AsyncWriteRent + ?Sized,
{
    let mut buf: Vec<u8> = Vec::with_capacity(BUF_SIZE);
    let mut transferred: u64 = 0;

    'r: loop {
        let (read_res, mut buf_read) = reader.read(buf).await;
        match read_res {
            Ok(0) => {
                // read closed
                break;
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                // retry
                buf = buf_read;
                continue;
            }
            Err(e) => {
                // should return error
                return Err(e);
            }
            Ok(_) => {
                // go write data
            }
        }

        'w: loop {
            let (write_res, buf_) = writer.write_all(buf_read).await;
            match write_res {
                Ok(0) => {
                    // write closed
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "write zero byte into writer",
                    ));
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                    // retry
                    buf_read = buf_;
                    continue 'w;
                }
                Err(e) => {
                    // should return error
                    return Err(e);
                }
                Ok(n) => {
                    // go read data
                    transferred += n as u64;
                    buf = buf_;
                    break;
                }
            }
        }
    }

    Ok(transferred)
}

/// Copy with splice.
#[cfg(all(target_os = "linux", feature = "splice"))]
pub async fn zero_copy<SRC: crate::io::as_fd::AsReadFd, DST: crate::io::as_fd::AsWriteFd>(
    reader: &mut SRC,
    writer: &mut DST,
) -> io::Result<u64> {
    use crate::{
        driver::op::Op,
        io::splice::{SpliceDestination, SpliceSource},
    };

    let (mut pr, mut pw) = new_pipe()?;
    let mut transferred: u64 = 0;
    loop {
        let mut to_write = reader.splice_to_pipe(&mut pw, BUF_SIZE as u32).await?;
        if to_write == 0 {
            break;
        }
        transferred += to_write as u64;
        while to_write > 0 {
            let written = writer.splice_from_pipe(&mut pr, to_write).await?;
            to_write -= written;
        }
    }
    Ok(transferred)
}

/// Copy with splice to pipe.
#[cfg(all(target_os = "linux", feature = "splice"))]
pub async fn zero_copy_to_pipe<SRC: crate::io::as_fd::AsReadFd>(
    reader: &mut SRC,
    writer: &mut Pipe,
) -> io::Result<u64> {
    use crate::{
        driver::op::Op,
        io::splice::{SpliceDestination, SpliceSource},
        net::Pipe,
    };

    let mut transferred: u64 = 0;
    loop {
        let written = reader.splice_to_pipe(writer, BUF_SIZE as u32).await?;
        if written == 0 {
            break;
        }
        transferred += written as u64;
    }
    Ok(transferred)
}

/// Copy with splice from pipe.
#[cfg(all(target_os = "linux", feature = "splice"))]
pub async fn zero_copy_from_pipe<DST: crate::io::as_fd::AsWriteFd>(
    reader: &mut Pipe,
    writer: &mut DST,
) -> io::Result<u64> {
    use crate::{
        driver::op::Op,
        io::splice::{SpliceDestination, SpliceSource},
    };

    let mut transferred: u64 = 0;
    loop {
        let written = writer.splice_from_pipe(reader, BUF_SIZE as u32).await?;
        if written == 0 {
            break;
        }
        transferred += written as u64;
    }
    Ok(transferred)
}

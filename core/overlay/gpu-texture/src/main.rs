use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use glint_gpu_texture::{
    CMD_INIT, CMD_SHUTDOWN, CMD_UPLOAD, GpuLuid, STATUS_ERR, STATUS_OK, SharedTextureUploader,
    read_i32_le_at, read_u32_le, read_u32_le_at,
};

fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    reader
        .read_exact(buf)
        .context("unexpected EOF while reading request")
}

fn write_response(writer: &mut impl Write, status: u8, payload: &[u8]) -> Result<()> {
    let len = 1 + payload.len();
    writer.write_all(&(len as u32).to_le_bytes())?;
    writer.write_all(&[status])?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

fn write_ok(writer: &mut impl Write, payload: &[u8]) -> Result<()> {
    write_response(writer, STATUS_OK, payload)
}

fn write_err(writer: &mut impl Write, message: &str) -> Result<()> {
    write_response(writer, STATUS_ERR, message.as_bytes())
}

fn handle_request(
    uploader: &mut Option<SharedTextureUploader>,
    writer: &mut impl Write,
    payload: &[u8],
) -> Result<()> {
    let cmd = *payload.first().context("empty request")?;

    match cmd {
        CMD_INIT => {
            if payload.len() < 9 {
                bail!("init payload too short");
            }
            let low = read_u32_le_at(payload, 1)?;
            let high = read_i32_le_at(payload, 5)?;
            *uploader = Some(SharedTextureUploader::new(GpuLuid { low, high })?);
            write_ok(writer, &[])?;
        }

        CMD_UPLOAD => {
            let uploader = uploader
                .as_mut()
                .context("upload before init — send init first")?;
            if payload.len() < 9 {
                bail!("upload payload too short");
            }
            let width = read_u32_le_at(payload, 1)?;
            let height = read_u32_le_at(payload, 5)?;
            let data = &payload[9..];
            let handle = uploader.upload(width, height, data)?;
            write_ok(writer, &handle.to_le_bytes())?;
        }

        CMD_SHUTDOWN => {
            write_ok(writer, &[])?;
            bail!("shutdown requested");
        }

        other => bail!("unknown command {other}"),
    }

    Ok(())
}

fn main() -> Result<()> {
    run_server()
}

fn run_server() -> Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut uploader: Option<SharedTextureUploader> = None;
    let mut len_buf = [0u8; 4];

    loop {
        if read_exact(&mut stdin, &mut len_buf).is_err() {
            break;
        }
        let len = read_u32_le(&len_buf)? as usize;
        if len == 0 {
            write_err(&mut stdout, "zero-length request")?;
            continue;
        }

        let mut payload = vec![0u8; len];
        if let Err(err) = read_exact(&mut stdin, &mut payload) {
            let _ = write_err(&mut stdout, &format!("{err:#}"));
            break;
        }

        match handle_request(&mut uploader, &mut stdout, &payload) {
            Ok(()) => {}
            Err(err) if err.to_string().contains("shutdown requested") => break,
            Err(err) => {
                let _ = write_err(&mut stdout, &format!("{err:?}"));
            }
        }
    }

    Ok(())
}

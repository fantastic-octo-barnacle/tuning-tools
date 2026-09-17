//! Serial ports: the firmware's USB CDC tuning link, or a UART adapter.

use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use serde::Serialize;
use serialport::{SerialPort, SerialPortType};

use crate::{ByteStream, CarrierError, Result};

/// USB product string the firmware's tuning link reports.
pub const TELEMETRY_PRODUCT: &str = "rm-telemetry";
/// How long one [`ByteStream::read`] waits for bytes.
const READ_TIMEOUT: Duration = Duration::from_millis(5);
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub path: String,
    /// USB product string, when the port is a USB device
    pub product: Option<String>,
    pub serial: Option<String>,
    /// The port is a firmware tuning link
    pub telemetry: bool,
}

/// Serial ports, tuning links first. On macOS only the `cu.` callout devices
/// are listed; their `tty.` twins block on open until carrier detect.
pub fn list_ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.port_name.starts_with("/dev/tty."))
        .map(|p| {
            let (product, serial) = match p.port_type {
                SerialPortType::UsbPort(usb) => (usb.product, usb.serial_number),
                _ => (None, None),
            };
            PortInfo {
                telemetry: product.as_deref() == Some(TELEMETRY_PRODUCT),
                path: p.port_name,
                product,
                serial,
            }
        })
        .collect();
    ports.sort_by(|a, b| b.telemetry.cmp(&a.telemetry).then(a.path.cmp(&b.path)));
    ports
}

pub struct SerialStream {
    port: Box<dyn SerialPort>,
    path: String,
}

impl SerialStream {
    /// Open `path`. The baud rate is ignored by USB CDC and used by a UART.
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|e| CarrierError::Port {
                port: path.to_string(),
                reason: e.to_string(),
            })?;
        Ok(Self {
            port,
            path: path.to_string(),
        })
    }
}

impl ByteStream for SerialStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(0),
            Err(e) => Err(CarrierError::Stream(format!("{}: {e}", self.path))),
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        let result = self
            .port
            .set_timeout(WRITE_TIMEOUT)
            .map_err(std::io::Error::from)
            .and_then(|()| self.port.write_all(bytes));
        let _ = self.port.set_timeout(READ_TIMEOUT);
        result.map_err(|e| CarrierError::Stream(format!("{}: {e}", self.path)))
    }
}

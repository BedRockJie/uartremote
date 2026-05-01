use crate::protocol::{DataBits, Parity, SerialConfig, SerialPortInfo, StopBits};
use serialport::SerialPortType;
use std::time::Duration;

pub fn list_serial_ports() -> crate::Result<Vec<SerialPortInfo>> {
    let ports = serialport::available_ports()
        .map_err(|error| crate::CoreError::Serial(error.to_string()))?;

    Ok(ports
        .into_iter()
        .map(|port| SerialPortInfo {
            port_name: port.port_name,
            port_type: describe_port_type(&port.port_type),
        })
        .collect())
}

pub fn open_serial_port(config: &SerialConfig) -> crate::Result<Box<dyn serialport::SerialPort>> {
    serialport::new(&config.port, config.baud_rate)
        .data_bits(to_serial_data_bits(config.data_bits))
        .stop_bits(to_serial_stop_bits(config.stop_bits))
        .parity(to_serial_parity(config.parity))
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|error| crate::CoreError::Serial(error.to_string()))
}

fn describe_port_type(port_type: &SerialPortType) -> String {
    match port_type {
        SerialPortType::UsbPort(info) => {
            let product = info.product.as_deref().unwrap_or("USB serial");
            format!("usb:{product}")
        }
        SerialPortType::PciPort => "pci".to_string(),
        SerialPortType::BluetoothPort => "bluetooth".to_string(),
        SerialPortType::Unknown => "unknown".to_string(),
    }
}

fn to_serial_data_bits(value: DataBits) -> serialport::DataBits {
    match value {
        DataBits::Five => serialport::DataBits::Five,
        DataBits::Six => serialport::DataBits::Six,
        DataBits::Seven => serialport::DataBits::Seven,
        DataBits::Eight => serialport::DataBits::Eight,
    }
}

fn to_serial_stop_bits(value: StopBits) -> serialport::StopBits {
    match value {
        StopBits::One => serialport::StopBits::One,
        StopBits::Two => serialport::StopBits::Two,
    }
}

fn to_serial_parity(value: Parity) -> serialport::Parity {
    match value {
        Parity::None => serialport::Parity::None,
        Parity::Odd => serialport::Parity::Odd,
        Parity::Even => serialport::Parity::Even,
    }
}

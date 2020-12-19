use smoltcp::{
    iface::EthernetInterface,
    phy,
    socket::{SocketHandle, SocketRef, TcpSocket},
};

use crate::random::Random;

const RX_BUF_SZ: usize = 4096;
const TX_BUF_SZ: usize = 4096;

pub trait TcpClient {
    fn set_socket_handle(&mut self, handle: SocketHandle);
    fn get_socket_handle(&mut self) -> SocketHandle;
    fn poll<DeviceT>(
        &mut self,
        interface: &mut EthernetInterface<DeviceT>,
        socket: SocketRef<TcpSocket>,
        random: &mut Random,
    ) where
        DeviceT: for<'d> phy::Device<'d>;
}

pub struct TcpClientStore {
    pub rx_buffer: [u8; RX_BUF_SZ],
    pub tx_buffer: [u8; TX_BUF_SZ],
}

impl TcpClientStore {
    pub fn new() -> Self {
        TcpClientStore {
            rx_buffer: [0; RX_BUF_SZ],
            tx_buffer: [0; TX_BUF_SZ],
        }
    }
}

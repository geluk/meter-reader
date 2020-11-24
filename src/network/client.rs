use smoltcp::{
    iface::EthernetInterface,
    phy,
    socket::{SocketHandle, SocketRef, TcpSocket},
};

use crate::random::Random;

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
    pub rx_buffer: [u8; 4096],
    pub tx_buffer: [u8; 4096],
}

impl TcpClientStore {
    pub fn new() -> Self {
        TcpClientStore {
            rx_buffer: [0; 4096],
            tx_buffer: [0; 4096],
        }
    }
}

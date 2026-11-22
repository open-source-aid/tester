use core::future::Future;
use core::time::Duration;
use std::path::Path;
use std::sync::mpsc;

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use btleplug::Error;
use futures::StreamExt;
use pump_shared::{BleOp, BleResult, Notification, NotifyKind, SimDevice, Timestamp, Uuid};
use serialport::SerialPort;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::{status, Advertised, Console};

const OP: Duration = Duration::from_secs(10);
const CONNECT: Duration = Duration::from_secs(20);
const SCAN_POLL: Duration = Duration::from_millis(200);
const CONSOLE_BAUD: u32 = 115_200;

pub fn open_console(path: &Path) -> serialport::Result<Console<Box<dyn SerialPort>>> {
    let port = serialport::new(path.to_string_lossy(), CONSOLE_BAUD)
        .timeout(Duration::from_secs(2))
        .open()?;
    Ok(Console::new(port))
}

pub struct Radio {
    runtime: Runtime,
    central: Link,
}

struct Link {
    adapter: Adapter,
    name: Advertised,
    peripheral: Option<Peripheral>,
    sender: mpsc::Sender<Notification>,
    forwarder: Option<JoinHandle<()>>,
    notifications: mpsc::Receiver<Notification>,
}

impl Radio {
    pub fn new(name: Advertised) -> Result<Self, Error> {
        let runtime = Runtime::new().map_err(|e| Error::RuntimeError(e.to_string()))?;
        let adapter = runtime.block_on(async {
            let adapters = Manager::new().await?.adapters().await?;
            adapters.into_iter().next().ok_or(Error::NoAdapterAvailable)
        })?;
        let (sender, notifications) = mpsc::channel();
        Ok(Self {
            runtime,
            central: Link {
                adapter,
                name,
                peripheral: None,
                sender,
                forwarder: None,
                notifications,
            },
        })
    }
}

impl SimDevice for Radio {
    fn handle(&mut self, op: &BleOp, _now: Timestamp) -> BleResult {
        self.runtime.block_on(self.central.perform(op))
    }

    fn poll_notification(&mut self, _now: Timestamp) -> Option<Notification> {
        self.central.notifications.try_recv().ok()
    }
}

fn uuid(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}

async fn timed<T>(limit: Duration, op: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::time::timeout(limit, op)
        .await
        .unwrap_or(Err(Error::TimedOut(limit)))
}

impl Link {
    async fn perform(&mut self, op: &BleOp) -> BleResult {
        let result = match op {
            BleOp::Connect { .. } => timed(CONNECT, self.connect()).await,
            BleOp::Disconnect => self.disconnect().await,
            BleOp::RequestFastInterval => Ok(BleResult::Done),
            BleOp::Read {
                service,
                characteristic,
            } => match self.find(*service, *characteristic) {
                Ok((peripheral, found)) => timed(OP, peripheral.read(&found))
                    .await
                    .map(BleResult::Data),
                Err(missing) => Ok(missing),
            },
            BleOp::Write {
                service,
                characteristic,
                data,
            } => match self.find(*service, *characteristic) {
                Ok((peripheral, found)) => {
                    timed(OP, peripheral.write(&found, data, WriteType::WithResponse))
                        .await
                        .map(|()| BleResult::Done)
                }
                Err(missing) => Ok(missing),
            },
            BleOp::Subscribe {
                service,
                characteristic,
                kind,
            } => match self.find(*service, *characteristic) {
                Ok((_, found)) if !found.properties.contains(property(*kind)) => {
                    Ok(BleResult::Failed)
                }
                Ok((peripheral, found)) => timed(OP, peripheral.subscribe(&found))
                    .await
                    .map(|()| BleResult::Done),
                Err(missing) => Ok(missing),
            },
            BleOp::Unsubscribe {
                service,
                characteristic,
            } => match self.find(*service, *characteristic) {
                Ok((peripheral, found)) => timed(OP, peripheral.unsubscribe(&found))
                    .await
                    .map(|()| BleResult::Done),
                Err(missing) => Ok(missing),
            },
        };
        match result {
            Ok(done) => done,
            Err(error) => self.failure(error).await,
        }
    }

    async fn failure(&self, error: Error) -> BleResult {
        let connected = match &self.peripheral {
            Some(peripheral) => peripheral.is_connected().await.unwrap_or(false),
            None => false,
        };
        match error {
            Error::TimedOut(_) => BleResult::Timeout,
            Error::NotConnected | Error::DeviceNotFound => BleResult::Disconnected,
            _ if !connected => BleResult::Disconnected,
            other => status(&other.to_string()),
        }
    }

    fn find(
        &self,
        service: Uuid,
        characteristic: Uuid,
    ) -> Result<(&Peripheral, Characteristic), BleResult> {
        let peripheral = self.peripheral.as_ref().ok_or(BleResult::Disconnected)?;
        peripheral
            .characteristics()
            .into_iter()
            .find(|c| {
                uuid(c.service_uuid.as_bytes()) == service
                    && uuid(c.uuid.as_bytes()) == characteristic
            })
            .map(|found| (peripheral, found))
            .ok_or(BleResult::Status(0x0A))
    }

    async fn connect(&mut self) -> Result<BleResult, Error> {
        self.peripheral = None;
        let peripheral = loop {
            self.adapter.clear_peripherals().await?;
            let candidate = self.scan().await?;
            if candidate.connect().await.is_ok() {
                break candidate;
            }
            tokio::time::sleep(SCAN_POLL).await;
        };
        let peripheral = self.peripheral.insert(peripheral);
        peripheral.discover_services().await?;
        let mut stream = peripheral.notifications().await?;
        let sender = self.sender.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(value) = stream.next().await {
                let notification = Notification {
                    service: uuid(value.service_uuid.as_bytes()),
                    characteristic: uuid(value.uuid.as_bytes()),
                    data: value.value,
                };
                if sender.send(notification).is_err() {
                    break;
                }
            }
        });
        if let Some(old) = self.forwarder.replace(forwarder) {
            old.abort();
        }
        Ok(BleResult::Done)
    }

    async fn scan(&self) -> Result<Peripheral, Error> {
        self.adapter.start_scan(ScanFilter::default()).await?;
        loop {
            for candidate in self.adapter.peripherals().await? {
                let properties = candidate.properties().await?.unwrap_or_default();
                if self.name.is(properties.local_name.as_deref()) {
                    self.adapter.stop_scan().await?;
                    return Ok(candidate);
                }
            }
            tokio::time::sleep(SCAN_POLL).await;
        }
    }

    async fn disconnect(&self) -> Result<BleResult, Error> {
        if let Some(peripheral) = &self.peripheral {
            if peripheral.is_connected().await? {
                timed(OP, peripheral.disconnect()).await?;
            }
        }
        Ok(BleResult::Done)
    }
}

fn property(kind: NotifyKind) -> CharPropFlags {
    match kind {
        NotifyKind::Notification => CharPropFlags::NOTIFY,
        NotifyKind::Indication => CharPropFlags::INDICATE,
    }
}

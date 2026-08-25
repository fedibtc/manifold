use std::collections::BTreeMap;
use std::net::{TcpListener, UdpSocket};

use serde::{Deserialize, Serialize};

/// The lowest port number to try.
const LOW: u16 = 10_000;

/// The highest port number to try.
const HIGH: u16 = 32_000;

type UnixTimestamp = u64;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case")]
struct RangeData {
    /// Port range size.
    size: u16,

    /// Unix timestamp when this range expires.
    expires: UnixTimestamp,
}

fn default_next() -> u16 {
    LOW
}

/// Persistent allocator state stored under the data directory lock.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct RootData {
    /// Next port to try.
    #[serde(default = "default_next")]
    next: u16,

    /// Map of port ranges keyed by the first port in the range.
    keys: BTreeMap<u16, RangeData>,
}

impl Default for RootData {
    fn default() -> Self {
        Self {
            next: LOW,
            keys: Default::default(),
        }
    }
}

impl RootData {
    /// Find and reserve a free port range.
    pub(crate) fn get_free_port_range(&mut self, range_size: u16) -> u16 {
        self.reclaim();

        let mut base_port: u16 = self.next;
        'retry: loop {
            if base_port > HIGH {
                self.reclaim();
                base_port = LOW;
            }

            let range = base_port..base_port + range_size;
            if let Some(next_port) = self.contains(range.clone()) {
                base_port = next_port;
                continue 'retry;
            }

            for port in range.clone() {
                match (
                    TcpListener::bind(("127.0.0.1", port)),
                    UdpSocket::bind(("127.0.0.1", port)),
                ) {
                    (Err(_), _) | (_, Err(_)) => {
                        base_port = port + 1;
                        continue 'retry;
                    }
                    (Ok(tcp), Ok(udp)) => (tcp, udp),
                };
            }

            self.insert(range);
            return base_port;
        }
    }

    /// Remove expired entries from the map.
    fn reclaim(&mut self) {
        let now = Self::now_ts();
        self.keys.retain(|_k, v| now < v.expires);
    }

    /// Check if `range` conflicts with an already reserved range.
    fn contains(&self, range: std::ops::Range<u16>) -> Option<u16> {
        self.keys.range(..range.end).next_back().and_then(|(k, v)| {
            let start = *k;
            let end = start + v.size;

            if start < range.end && range.start < end {
                Some(end)
            } else {
                None
            }
        })
    }

    fn insert(&mut self, range: std::ops::Range<u16>) {
        const ALLOCATION_TIME_SECS: u64 = 120;

        assert!(self.contains(range.clone()).is_none());
        self.keys.insert(
            range.start,
            RangeData {
                size: range.len() as u16,
                expires: Self::now_ts() + ALLOCATION_TIME_SECS,
            },
        );
        self.next = range.end;
    }

    fn now_ts() -> UnixTimestamp {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must not be before Unix epoch")
            .as_secs()
    }
}

#[test]
fn root_data_sanity() {
    let mut r = RootData::default();

    r.insert(2..4);
    r.insert(6..8);
    r.insert(100..108);
    assert_eq!(r.contains(0..2), None);
    assert_eq!(r.contains(0..3), Some(4));
    assert_eq!(r.contains(2..4), Some(4));
    assert_eq!(r.contains(3..4), Some(4));
    assert_eq!(r.contains(3..5), Some(4));
    assert_eq!(r.contains(4..6), None);
    assert_eq!(r.contains(0..10), Some(8));
    assert_eq!(r.contains(6..10), Some(8));
    assert_eq!(r.contains(7..8), Some(8));
    assert_eq!(r.contains(8..10), None);
}

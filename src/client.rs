use serde_json::{json, Value};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const DEFAULT_BATCH_SIZE: u32 = 1000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A small client for the local/NAS/remote JSON-line service protocol.
#[derive(Debug, Clone)]
pub struct ServiceClient {
    address: String,
    timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct CompleteSearch {
    /// Every matching result, accumulated from all transport batches.
    pub results: Vec<Value>,
    /// Total number of matches reported by the service.
    pub total: u64,
}

impl ServiceClient {
    pub fn new(address: impl Into<String>) -> Self {
        Self { address: address.into(), timeout: DEFAULT_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn status(&self) -> Result<Value, Box<dyn Error>> {
        self.request(json!({"command": "status"}))
    }

    pub fn set_stop_on_logout(&self, enabled: bool) -> Result<Value, Box<dyn Error>> {
        self.request(json!({"command": "set_stop_on_logout", "enabled": enabled}))
    }

    pub fn index(&self, paths: &[String]) -> Result<Value, Box<dyn Error>> {
        self.request(json!({"command": "index", "paths": paths}))
    }

    /// Fetches every match automatically. The service's 1000-record limit is
    /// only a transport batch size; callers receive one continuous collection.
    pub fn search_all(
        &self,
        query: &str,
        sort: &str,
        order: &str,
    ) -> Result<CompleteSearch, Box<dyn Error>> {
        let mut offset = 0_u64;
        let mut expected_total = None;
        let mut results = Vec::new();

        loop {
            let response = self.request(json!({
                "command": "search",
                "query": query,
                "limit": DEFAULT_BATCH_SIZE,
                "offset": offset,
                "sort": sort,
                "order": order
            }))?;

            let total = response.get("total").and_then(Value::as_u64)
                .ok_or("Search response is missing a valid total")?;
            match expected_total {
                Some(expected) if expected != total =>
                    return Err("Search result total changed while fetching batches".into()),
                None => expected_total = Some(total),
                _ => {}
            }

            let batch = response.get("results").and_then(Value::as_array)
                .ok_or("Search response is missing a results array")?;
            if batch.len() > DEFAULT_BATCH_SIZE as usize {
                return Err("Service returned a batch larger than the requested limit".into());
            }
            if batch.is_empty() && response.get("has_more").and_then(Value::as_bool) == Some(true) {
                return Err("Service reported more results but returned an empty batch".into());
            }

            let received = batch.len() as u64;
            results.extend(batch.iter().cloned());
            offset = offset.checked_add(received)
                .ok_or("Search result offset overflowed")?;

            let has_more = response.get("has_more").and_then(Value::as_bool)
                .ok_or("Search response is missing has_more")?;
            if !has_more {
                if offset != total {
                    return Err(format!(
                        "Incomplete search response: received {offset} of {total} results"
                    ).into());
                }
                return Ok(CompleteSearch { results, total });
            }
            if offset >= total {
                return Err("Service reported more results after the reported total".into());
            }
        }
    }

    fn request(&self, request: Value) -> Result<Value, Box<dyn Error>> {
        let address = self.address.to_socket_addrs()?.next()
            .ok_or("Service address did not resolve")?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.flush()?;

        let mut response_line = String::new();
        BufReader::new(stream).read_line(&mut response_line)?;
        if response_line.is_empty() || !response_line.ends_with('\n') {
            return Err("Service closed the connection without a complete response".into());
        }
        let envelope: Value = serde_json::from_str(&response_line)?;
        if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
            let message = envelope.get("error").and_then(Value::as_str)
                .unwrap_or("Unknown service error");
            return Err(message.to_owned().into());
        }
        envelope.get("data").cloned().ok_or_else(|| "Service response is missing data".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn search_all_accumulates_every_batch_without_duplicates() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["command"], "search");
                assert_eq!(request["limit"], 1000);
                let offset = request["offset"].as_u64().unwrap();
                let count = if offset == 0 { 1000 } else { 5 };
                let results: Vec<Value> = (offset..offset + count)
                    .map(|index| json!({
                        "name": format!("report-{index:04}.txt"),
                        "size_bytes": index
                    }))
                    .collect();
                let response = json!({
                    "ok": true,
                    "data": {
                        "results": results,
                        "total": 1005,
                        "limit": 1000,
                        "offset": offset,
                        "has_more": offset + count < 1005
                    }
                });
                serde_json::to_writer(&mut stream, &response).unwrap();
                stream.write_all(b"\n").unwrap();
            }
        });

        let complete = ServiceClient::new(address.to_string())
            .search_all("name:report", "name", "asc").unwrap();
        server.join().unwrap();

        assert_eq!(complete.total, 1005);
        assert_eq!(complete.results.len(), 1005);
        assert_eq!(complete.results.first().unwrap()["name"], "report-0000.txt");
        assert_eq!(complete.results.last().unwrap()["name"], "report-1004.txt");
    }

    #[test]
    fn rejects_inconsistent_or_incomplete_batch_sequences() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            serde_json::to_writer(&mut stream, &json!({
                "ok": true,
                "data": {
                    "results": [{"name": "one.txt"}],
                    "total": 2,
                    "has_more": false
                }
            })).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let error = ServiceClient::new(address.to_string())
            .search_all("name:one", "name", "asc").unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("Incomplete search response"));
    }
}

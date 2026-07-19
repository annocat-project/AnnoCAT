use std::io::Read;
use std::time::Duration;

const HTTP_RANGE_BYTES: u64 = 64 * 1024 * 1024;
const HTTP_RECONNECT_ATTEMPTS: u32 = 4;

pub(super) struct ReconnectingRangeReader {
    client: reqwest::blocking::Client,
    source_url: String,
    resource_id: String,
    chromosome: String,
    current: u64,
    absolute_end: u64,
    object_bytes: u64,
    expected_etag: Option<String>,
    expected_last_modified: Option<String>,
    response: Option<reqwest::blocking::Response>,
    response_end: u64,
}

impl ReconnectingRangeReader {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        source_url: &str,
        resource_id: &str,
        chromosome: &str,
        absolute_start: u64,
        absolute_end: u64,
        object_bytes: u64,
        expected_etag: Option<&str>,
        expected_last_modified: Option<&str>,
    ) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|error| format!("cannot create resumable preparation client: {error}"))?;
        Ok(Self {
            client,
            source_url: source_url.into(),
            resource_id: resource_id.into(),
            chromosome: chromosome.into(),
            current: absolute_start,
            absolute_end,
            object_bytes,
            expected_etag: expected_etag.map(str::to_owned),
            expected_last_modified: expected_last_modified.map(str::to_owned),
            response: None,
            response_end: absolute_start,
        })
    }

    fn open_chunk(&mut self) -> Result<(), String> {
        let chunk_end = self
            .current
            .saturating_add(HTTP_RANGE_BYTES.saturating_sub(1))
            .min(self.absolute_end);
        let response = self
            .client
            .get(&self.source_url)
            .header(
                reqwest::header::RANGE,
                format!("bytes={}-{}", self.current, chunk_end),
            )
            .send()
            .map_err(|error| format!("range request failed: {error}"))?;
        let expected_bytes = chunk_end - self.current + 1;
        let valid_full_response = self.current == 0
            && chunk_end + 1 == self.object_bytes
            && response.status().is_success()
            && response.content_length() == Some(expected_bytes);
        let expected_range = format!("bytes {}-{chunk_end}/{}", self.current, self.object_bytes);
        let valid_range_response = response.status() == reqwest::StatusCode::PARTIAL_CONTENT
            && response.content_length() == Some(expected_bytes)
            && response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                == Some(expected_range.as_str());
        if !valid_full_response && !valid_range_response {
            return Err(format!(
                "HTTP {} returned incompatible metadata for bytes {}-{chunk_end}",
                response.status(),
                self.current
            ));
        }
        super::validate_optional_header(
            response.headers(),
            reqwest::header::LAST_MODIFIED,
            self.expected_last_modified.as_deref(),
            "Last-Modified",
        )?;
        if self
            .expected_etag
            .as_deref()
            .is_none_or(|value| !value.starts_with("md5:"))
        {
            super::validate_optional_header(
                response.headers(),
                reqwest::header::ETAG,
                self.expected_etag.as_deref(),
                "ETag",
            )?;
        }
        self.response = Some(response);
        self.response_end = chunk_end;
        Ok(())
    }

    fn reconnect(&mut self, attempt: u32) {
        let delay = 1_u64 << attempt.min(3);
        if let Ok(mut state) = super::state::live_state().lock() {
            state.throughput_bytes_per_second = 0.0;
            state.detail = format!(
                "{} chromosome {}: reconnecting at source byte {} in {delay}s",
                self.resource_id, self.chromosome, self.current
            );
        }
        std::thread::sleep(Duration::from_secs(delay));
    }
}

impl Read for ReconnectingRangeReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.current > self.absolute_end {
            return Ok(0);
        }
        let mut attempts = 0_u32;
        loop {
            if self.response.is_none() {
                if let Err(error) = self.open_chunk() {
                    if attempts >= HTTP_RECONNECT_ATTEMPTS {
                        return Err(std::io::Error::other(format!(
                            "hybrid range reconnect failed at source byte {}: {error}",
                            self.current
                        )));
                    }
                    self.reconnect(attempts);
                    attempts += 1;
                    continue;
                }
            }
            let remaining = self.response_end - self.current + 1;
            let bounded = output.len().min(remaining as usize);
            let result = self
                .response
                .as_mut()
                .expect("response was opened")
                .read(&mut output[..bounded]);
            match result {
                Ok(0) => {
                    self.response = None;
                    if self.current > self.response_end {
                        attempts = 0;
                        if self.current > self.absolute_end {
                            return Ok(0);
                        }
                        continue;
                    }
                    let error = "response ended before its advertised byte range";
                    if attempts >= HTTP_RECONNECT_ATTEMPTS {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            error,
                        ));
                    }
                    self.reconnect(attempts);
                    attempts += 1;
                }
                Ok(read) => {
                    self.current = self.current.saturating_add(read as u64);
                    if self.current > self.response_end {
                        self.response = None;
                    }
                    return Ok(read);
                }
                Err(error) => {
                    self.response = None;
                    if attempts >= HTTP_RECONNECT_ATTEMPTS {
                        return Err(error);
                    }
                    self.reconnect(attempts);
                    attempts += 1;
                }
            }
        }
    }
}

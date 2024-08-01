use reqwest;
use serde;
use std::env;
use std::io::ErrorKind as IoErrorKind;

#[derive(serde::Deserialize)]
struct DnsOverHttpsResponse {
    #[serde(rename(deserialize = "Answer"))]
    pub answer: Vec<DnsOverHttpsAnswer>,
}

#[derive(serde::Deserialize)]
struct DnsOverHttpsAnswer {
    pub data: String,
}

fn strip_quotes(data: &String) -> String {
    let v1 = match data.strip_prefix('"') {
        None => data.as_str(),
        Some(x) => x,
    };
    String::from(match v1.strip_suffix('"') {
        None => v1,
        Some(x) => x,
    })
}

pub enum DnsOverHttpsServer {
    Google,
    Cloudflare,
}

impl DnsOverHttpsServer {
    pub fn get_dns_url(self: &DnsOverHttpsServer) -> &str {
        match self {
            DnsOverHttpsServer::Google => "https://dns.google/resolve",
            DnsOverHttpsServer::Cloudflare => "https://cloudflare-dns.com/dns-query",
        }
    }
}

pub async fn resolve_dns_record_https(
    record: &str,
    record_type: &str,
    dns_server: DnsOverHttpsServer,
) -> std::io::Result<Vec<String>> {
    let result = reqwest::Client::new()
        .get(dns_server.get_dns_url())
        .query(&[("name", record), ("type", record_type)])
        .header(reqwest::header::ACCEPT, "application/dns-json")
        .send()
        .await
        .map_err(|_| std::io::Error::new(IoErrorKind::Other, "Couldn't fetch DNS record."))?
        .json::<DnsOverHttpsResponse>()
        .await
        .map_err(|_| std::io::Error::new(IoErrorKind::Other, "Couldn't fetch DNS record."))?
        .answer
        .iter()
        .map(|a| strip_quotes(&a.data))
        .collect();
    Ok(result)
}

pub async fn resolve_txt_record_to_string_array_https(
    record: &str,
    dns_server: DnsOverHttpsServer,
) -> std::io::Result<Vec<String>> {
    resolve_dns_record_https(record, "TXT", dns_server).await
}

pub fn should_use_dns_over_https() -> bool {
    matches!(env::var("YA_USE_HTTPS_DNS_RESOLVER"), Ok(value) if value == "1")
}

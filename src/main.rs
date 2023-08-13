use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::{OffsetName, Tz, TZ_VARIANTS};
use clap::Parser;
use http_auth::PasswordClient;
use serde::{Deserialize, Deserializer};
use std::{
    convert::TryFrom as _,
    time::{SystemTime, UNIX_EPOCH},
};
use ureq::{builder, Agent, MiddlewareNext, Request, Response};

#[derive(Parser)]
#[command(author, version, about, long_about = None)] // Read from `Cargo.toml`
struct Cli {
    #[arg(long, required = true)]
    username: String,

    #[arg(long, required = true)]
    password: String,

    #[arg(long, required = true)]
    url: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct InvertersResponse {
    serial_number: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    last_report_date: DateTime<Utc>,
    last_report_watts: i32,
    max_report_watts: i32,
    /*{
    "0": {
        "serialNumber": "123456789012",
        "lastReportDate": 1688000000,
        "lastReportWatts": 123,
        "maxReportWatts": 234
    }
     */
}

#[derive(Deserialize, Debug)]
struct HomeResponse {
    /*
    {
    "software_build_epoch":1234567890,
    "is_nonvoy":false,
    "db_size":"12 MB",
    "db_percent_full":"1",
    "timezone":"America/New_York",
    "current_date":"01/01/2023",
    "current_time":"01:00",
    "network":{
        "web_comm":true,
        "ever_reported_to_enlighten":true,
        "last_enlighten_report_time":1234567890,
        "primary_interface":"wlan0",
        "interfaces":[
            {
                "type":"ethernet",
                "interface":"eth0",
                "mac":"00:11:22:33:44:55",
                "dhcp":true,"ip":"169.169.169.169",
                "signal_strength":0,
                "signal_strength_max":1,
                "carrier":false
            },{
                "signal_strength":1,
                "signal_strength_max":5,
                "type":"wifi",
                "interface":"wlan0",
                "mac":"11:22:33:44:55:66",
                "dhcp":true,
                "ip":"192.168.1.1",
                "carrier":true,
                "supported":true,
                "present":true,
                "configured":true,
                "status":"connected"
            }
        ]
    },
    "comm":{"num":1,"level":1},
    "alerts":[],
    "update_status":"satisfied"}
     */
    #[serde(with = "chrono::serde::ts_seconds")]
    software_build_epoch: DateTime<Utc>,

    current_date: String,

    current_time: String,

    timezone: String,

    #[serde(deserialize_with = "decode_memory_string")]
    db_size: i32,

    #[serde(deserialize_with = "string_to_i32")]
    db_percent_full: i32,

    network: HomeNetworkResponse,
    comm: HomeCommResponse,

    #[serde(deserialize_with = "decode_array_to_size")]
    alerts: usize,

    update_status: String,
}

#[derive(Deserialize, Debug)]
struct HomeNetworkResponse {
    #[serde(with = "chrono::serde::ts_seconds")]
    last_enlighten_report_time: DateTime<Utc>,
}

#[derive(Deserialize, Debug)]
struct HomeCommResponse {
    num: i32,
    level: i32,
}

fn decode_array_to_size<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    return Ok(serde_json::Value::deserialize(deserializer)?
        .as_array()
        .unwrap()
        .len());
}

fn string_to_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    return Ok(String::deserialize(deserializer)?.parse::<i32>().unwrap());
}

fn decode_memory_string<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let mut iter = s.split_ascii_whitespace();
    let mut number = iter.next().unwrap().parse::<i32>().unwrap();
    let multiple = iter.next().unwrap().to_ascii_uppercase();
    if multiple == "MB" {
        number *= 1024 * 1024;
    } else if multiple == "GB" {
        number *= 1024 * 1024 * 1024;
    } else if multiple == "KB" {
        number *= 1024;
    }
    return Ok(number);
}

fn get_home(agent: &Agent, url: &String) -> HomeResponse {
    let body = agent
        .get(&format!("{}/{}", url, "home.json"))
        .call()
        .unwrap();
    return body.into_json().unwrap();
}

fn home_to_influx(home: HomeResponse) {
    let now = SystemTime::now();
    let timestamp_nano = now.duration_since(UNIX_EPOCH).unwrap().as_nanos();
    println!(
        "software_build_date value={} {}",
        home.software_build_epoch.timestamp_nanos(),
        timestamp_nano
    );
    println!(
        "database total_size={},percent_full={} {}",
        home.db_size, home.db_percent_full, timestamp_nano
    );
    println!(
        "phone_home update_status=\"{}\",alerts={},last_report={} {}",
        home.update_status,
        home.alerts,
        home.network.last_enlighten_report_time.timestamp_nanos(),
        timestamp_nano
    );
    let zone = TZ_VARIANTS
        .into_iter()
        .filter(|t: &Tz| {
            let date = t.timestamp_nanos(timestamp_nano.try_into().unwrap());
            return date.offset().tz_id() == home.timezone;
        })
        .next()
        .unwrap();
    let device_datetime = zone
        .datetime_from_str(
            &format!("{} {}", home.current_date, home.current_time),
            "%m/%d/%Y %H:%M",
        )
        .unwrap();
    println!(
        "device_time_skew device_timestamp={} {}",
        device_datetime.timestamp_nanos(),
        timestamp_nano
    );
    println!("comm number={},level={} {}", home.comm.num, home.comm.level, timestamp_nano);
}

fn get_inverters(agent: &Agent, url: &String) -> Vec<InvertersResponse> {
    let body = agent
        .get(&format!("{}/{}", url, "api/v1/production/inverters"))
        .call()
        .unwrap();
    return body.into_json().unwrap();
}

fn inverters_to_influx(inverters: Vec<InvertersResponse>) {
    let now = SystemTime::now();
    let timestamp_nano = now.duration_since(UNIX_EPOCH).unwrap().as_nanos();
    for inverter in &inverters {
        println!(
            "inverter,serial_number=\"{}\" last_report={},last_watts={},max_watts={} {}",
            inverter.serial_number,
            inverter.last_report_date.timestamp_nanos(),
            inverter.last_report_watts,
            inverter.max_report_watts,
            timestamp_nano
        );
    }
}

fn get_auth_header(url: &String, username: &String, password: &String) -> String {
    let auth_response = ureq::get(&format!("{}/installer/setup/home", url))
        .call()
        .expect_err("Was expecting a 401 error from the server; no idea what to do now")
        .into_response()
        .unwrap();
    let response_header = auth_response.header("WWW-Authenticate").unwrap();
    let mut password_client = PasswordClient::try_from(response_header).unwrap();
    let auth_header = password_client
        .respond(&http_auth::PasswordParams {
            username,
            password,
            uri: "*",
            method: "GET",
            body: Some(&[]),
        })
        .unwrap();
    return auth_header;
}

fn main() {
    let cli = Cli::parse();
    let auth = get_auth_header(&cli.url, &cli.username, &cli.password);
    let basic_auth = move |req: Request, next: MiddlewareNext| -> Result<Response, ureq::Error> {
        return next.handle(req.set("Authorization", &auth));
    };
    let agent = builder().middleware(basic_auth).build();

    let home = get_home(&agent, &cli.url);
    home_to_influx(home);

    let inverters = get_inverters(&agent, &cli.url);
    inverters_to_influx(inverters);
}

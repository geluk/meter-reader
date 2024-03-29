#![allow(unused)]
#![no_std]

use core::{
    fmt::{Display, Write},
    num::ParseIntError,
};

use arrayvec::{ArrayString, ArrayVec};
use nom::{
    branch::alt,
    bytes::streaming::{tag, take, take_until, take_while1, take_while_m_n},
    character::{
        self,
        streaming::{char, crlf, digit1, hex_digit1},
    },
    combinator::{map_res, not, opt},
    error::{FromExternalError, ParseError},
    multi::{fill, many0_count},
    sequence::{delimited, pair, preceded, terminated},
    Compare, IResult, InputLength, InputTake, Parser,
};

const MAX_COSEM_PER_LINE: usize = 16;
const MAX_LINES_PER_TELEGRAM: usize = 32;

#[derive(Debug)]
pub struct Telegram {
    pub device_id: ArrayString<32>,
    pub lines: ArrayVec<Line, MAX_LINES_PER_TELEGRAM>,
    pub crc: u16,
}

impl Telegram {
    pub fn serialize<W: Write>(&self, writer: &mut W) {
        // Poor man's JSON
        write!(writer, "{{");
        let mut separator = "";
        for line in self.lines.iter() {
            match line {
                Line::Version(version) => {
                    write!(writer, "{}\"dsmr_version\": {}", separator, version);
                }
                Line::Timestamp(ts) => {
                    write!(writer, "{}\"timestamp\": \"{}\"", separator, ts);
                }
                Line::Consumed(tariff, power) => {
                    write!(
                        writer,
                        "{}\"tariff_{}_consumed\": {}",
                        separator, tariff, power
                    );
                }
                Line::Produced(tariff, power) => {
                    write!(
                        writer,
                        "{}\"tariff_{}_produced\": {}",
                        separator, tariff, power
                    );
                }
                Line::ActiveTariff(tariff) => {
                    write!(writer, "{}\"active_tariff\": {}", separator, tariff);
                }
                Line::TotalConsuming(power) => {
                    write!(writer, "{}\"total_consuming\": {}", separator, power);
                }
                Line::TotalProducing(power) => {
                    write!(writer, "{}\"total_producing\": {}", separator, power);
                }
                Line::PowerFailures(count) => {
                    write!(writer, "{}\"power_failures\": {}", separator, count);
                }
                Line::LongPowerFailures(count) => {
                    write!(writer, "{}\"long_power_failures\": {}", separator, count);
                }
                Line::VoltageSags(count) => {
                    write!(writer, "{}\"voltage_sags\": {}", separator, count);
                }
                Line::VoltageSwells(count) => {
                    write!(writer, "{}\"voltage_swells\": {}", separator, count);
                }
                Line::Current(phase, current) => {
                    write!(writer, "{}\"{}_current\": {}", separator, phase, current);
                }
                Line::Consuming(phase, power) => {
                    write!(writer, "{}\"{}_consuming\": {}", separator, phase, power);
                }
                Line::Producing(phase, power) => {
                    write!(writer, "{}\"{}_producing\": {}", separator, phase, power);
                }
                _ => {
                    // Do not write unknown lines
                }
            }
            separator = ",";
        }
        write!(writer, "}}");
    }
}

#[derive(Debug)]
pub struct RawLine<'a> {
    obis: [u8; 6],
    cosem: ArrayVec<&'a str, MAX_COSEM_PER_LINE>,
}

#[derive(Debug)]
pub struct Timestamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    dst: bool,
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )?;
        if self.dst {
            write!(f, "+02:00")
        } else {
            write!(f, "+01:00")
        }
    }
}

#[derive(Debug)]
pub enum Phase {
    L1,
    L2,
    L3,
}

impl Display for Phase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Phase::L1 => write!(f, "l1"),
            Phase::L2 => write!(f, "l2"),
            Phase::L3 => write!(f, "l3"),
        }
    }
}

#[derive(Debug)]
pub enum Line {
    Version(u8),
    Timestamp(Timestamp), // YYYY, MM, DD, HH, MM, SS
    EquipmentId,          // ID is not passed in for now, it's too unwieldy
    PowerFailureLog,      // Same here
    Consumed(u8, u32),    // tariff, Wh
    Produced(u8, u32),    // tariff, Wh
    ActiveTariff(u8),
    TotalConsuming(u32),    // W
    TotalProducing(u32),    // W
    PowerFailures(u32),     // count
    LongPowerFailures(u32), // count
    VoltageSags(u32),       // count
    VoltageSwells(u32),     // count
    Current(Phase, u32),    // phase number, A
    Consuming(Phase, u32),  // phase number, A
    Producing(Phase, u32),  // phase number, A
    UnknownObis([u8; 6]),
}

#[derive(Debug)]
pub struct CrcMismatch {
    calculated: u16,
    read: u16,
}

#[derive(Debug)]
pub enum TelegramParseError {
    CrcMismatch(CrcMismatch),
    InvalidUtf8,
    Incomplete,
    ParseError(usize, nom::error::ErrorKind),
}

pub fn parse(input: &[u8]) -> (usize, Result<Telegram, TelegramParseError>) {
    let input_str = match core::str::from_utf8(input) {
        Ok(res) => res,
        Err(err) => {
            // If we detect invalid UTF-8, discard as many bytes as is necessary to skip past the error.
            // error_len will be `None` if an unexpected end of a UTF-8 sequence is detected.
            // In that case, we most likely just need to wait for additional data, so we don't discard any bytes.
            return (
                err.error_len().map(|e| e + err.valid_up_to()).unwrap_or(0),
                Err(TelegramParseError::InvalidUtf8),
            );
        }
    };
    let line_buffer = ArrayVec::<Line, MAX_LINES_PER_TELEGRAM>::new();
    match telegram(input_str, line_buffer) {
        Ok((remaining, telegram)) => {
            let telegram_length = input_str.len() - remaining.len();

            let crc = crc16(&input[..telegram_length - 6]);

            let res = if telegram.crc != crc {
                Err(TelegramParseError::CrcMismatch(CrcMismatch {
                    calculated: crc,
                    read: telegram.crc,
                }))
            } else {
                Ok(telegram)
            };

            (input_str.len() - remaining.len(), res)
        }
        Err(nom::Err::Incomplete(err)) => (0, Err(TelegramParseError::Incomplete)),
        Err(nom::Err::Failure(err)) | Err(nom::Err::Error(err)) => {
            let pos = input_str.len() - err.input.len();
            (1, Err(TelegramParseError::ParseError(pos, err.code)))
        }
    }
}

fn telegram(
    input: &str,
    mut line_buffer: ArrayVec<Line, MAX_LINES_PER_TELEGRAM>,
) -> IResult<&str, Telegram> {
    let (input, device_id) = device_id(input)?;

    let device_id = ArrayString::from(device_id).map_err(|_| {
        nom::Err::Error(nom::error::Error {
            input,
            code: nom::error::ErrorKind::TooLarge,
        })
    })?;

    let crc_val: u16;
    let mut next_input = input;
    loop {
        if let (inp, Some(crc)) = opt(crc)(next_input)? {
            crc_val = crc;
            next_input = inp;
            break;
        }
        match line(next_input) {
            Ok((i, o)) => {
                next_input = i;
                line_buffer.try_push(o).map_err(|_| {
                    nom::Err::Error(nom::error::Error {
                        input,
                        code: nom::error::ErrorKind::TooLarge,
                    })
                })?;
            }
            Err(err) => {
                return Err(err);
            }
        }
    }

    Ok((
        next_input,
        Telegram {
            device_id,
            lines: line_buffer,
            crc: crc_val,
        },
    ))
}

fn device_id(input: &str) -> IResult<&str, &str> {
    delimited(tag("/"), take_until("\r\n"), pair(crlf, crlf))(input)
}

fn crc(input: &str) -> IResult<&str, u16> {
    let (next_input, crc) = delimited(tag("!"), hex_digit1, crlf)(input)?;

    let mut crc_hex = [0u8; 2];
    decode_hex(crc, &mut crc_hex[..]).map_err(nom::Err::Error)?;
    let crc = ((crc_hex[0] as u16) << 8) | crc_hex[1] as u16;
    Ok((next_input, crc))
}

fn line(input: &str) -> IResult<&str, Line> {
    fn map_cosem<'a, T, F>(
        val: Option<&&'a str>,
        func: F,
    ) -> Result<T, nom::Err<nom::error::Error<&'a str>>>
    where
        F: FnOnce(&'a str) -> IResult<&str, T>,
    {
        let cosem = *val.ok_or({
            nom::Err::Error(nom::error::Error {
                input: "",
                code: nom::error::ErrorKind::NonEmpty,
            })
        })?;
        let (_, res) = func(cosem)?;
        Ok(res)
    };
    let (input, raw) = raw_line(input)?;

    let line = match raw.obis {
        [1, 3, 0, 2, 8, 255] => Line::Version(map_cosem(raw.cosem.get(0), u8_complete(2))?),
        [0, 0, 1, 0, 0, 255] => Line::Timestamp(map_cosem(raw.cosem.get(0), timestamp)?),
        [0, 0, 96, 1, 1, 255] => Line::EquipmentId,
        [1, 0, 1, 8, tariff, 255] => {
            Line::Consumed(tariff, map_cosem(raw.cosem.get(0), fixed_point(6, 3))?)
        }
        [1, 0, 2, 8, tariff, 255] => {
            Line::Produced(tariff, map_cosem(raw.cosem.get(0), fixed_point(6, 3))?)
        }
        [0, 0, 96, 14, 0, 255] => Line::ActiveTariff(map_cosem(raw.cosem.get(0), u8_complete(4))?),
        [1, 0, 1, 7, 0, 255] => {
            Line::TotalConsuming(map_cosem(raw.cosem.get(0), fixed_point(2, 3))?)
        }
        [1, 0, 2, 7, 0, 255] => {
            Line::TotalProducing(map_cosem(raw.cosem.get(0), fixed_point(2, 3))?)
        }
        [0, 0, 96, 7, 21, 255] => {
            Line::PowerFailures(map_cosem(raw.cosem.get(0), u32_complete(5))?)
        }
        [0, 0, 96, 7, 9, 255] => {
            Line::LongPowerFailures(map_cosem(raw.cosem.get(0), u32_complete(5))?)
        }
        [1, 0, 99, 97, 0, 255] => Line::PowerFailureLog,
        [1, 0, 32, 32, 0, 255] => Line::VoltageSags(map_cosem(raw.cosem.get(0), u32_complete(5))?),
        [1, 0, 32, 36, 0, 255] => {
            Line::VoltageSwells(map_cosem(raw.cosem.get(0), u32_complete(5))?)
        }
        [1, 0, 31, 7, 0, 255] => {
            Line::Current(Phase::L1, map_cosem(raw.cosem.get(0), u32_complete(3))?)
        }
        [1, 0, 21, 7, 0, 255] => {
            Line::Producing(Phase::L1, map_cosem(raw.cosem.get(0), fixed_point(2, 3))?)
        }
        [1, 0, 22, 7, 0, 255] => {
            Line::Consuming(Phase::L1, map_cosem(raw.cosem.get(0), fixed_point(2, 3))?)
        }
        obis => Line::UnknownObis(obis),
    };
    Ok((input, line))
}

fn timestamp(input: &str) -> IResult<&str, Timestamp> {
    let (input, year) = u8_complete(2)(input)?;
    let (input, month) = u8_complete(2)(input)?;
    let (input, day) = u8_complete(2)(input)?;
    let (input, hour) = u8_complete(2)(input)?;
    let (input, minute) = u8_complete(2)(input)?;
    let (input, second) = u8_complete(2)(input)?;
    let (input, dst) = alt((char('S'), char('W')))(input)?;

    Ok((
        input,
        Timestamp {
            year: year as u16 + 2000,
            month,
            day,
            hour,
            minute,
            second,
            dst: dst == 'S',
        },
    ))
}

fn raw_line(input: &str) -> IResult<&str, RawLine> {
    let (mut input, obis) = obis_code(input)?;

    let mut cosem_arr = ArrayVec::<&str, MAX_COSEM_PER_LINE>::new();

    loop {
        let res = cosem::<nom::error::Error<_>>()(input);
        match res {
            Ok((next_input, cosem)) => {
                input = next_input;
                cosem_arr.try_push(cosem).map_err(|_| {
                    nom::Err::Error(nom::error::Error {
                        input,
                        code: nom::error::ErrorKind::TooLarge,
                    })
                })?;
            }
            Err(e @ nom::Err::Incomplete(_)) => {
                return Err(e);
            }
            Err(err) => {
                break;
            }
        }
    }
    let (input, _) = crlf(input)?;
    Ok((
        input,
        RawLine {
            obis,
            cosem: cosem_arr,
        },
    ))
}

fn obis_code(input: &str) -> IResult<&str, [u8; 6]> {
    let (input, obis_a) = terminated(u8, tag("-"))(input)?;
    let (input, obis_b) = terminated(u8, tag(":"))(input)?;
    let (input, obis_c) = terminated(u8, tag("."))(input)?;
    let (input, obis_d) = terminated(u8, tag("."))(input)?;
    let (input, obis_e) = u8(input)?;

    // According to the OBIS spec, value group F is optional and should be interpreted as 255 if missing.
    let (input, obis_f) = opt(preceded(tag("."), u8))(input)?;
    let obis_f = obis_f.unwrap_or(255);

    Ok((input, [obis_a, obis_b, obis_c, obis_d, obis_e, obis_f]))
}

fn cosem<'a, E: ParseError<&'a str>>() -> impl FnMut(&'a str) -> IResult<&str, &str, E> {
    delimited(tag("("), take_until(")"), tag(")"))
}

fn u8(input: &str) -> IResult<&str, u8> {
    map_res(digit1, |s: &str| s.parse())(input)
}

fn u8_complete<'a, E>(digits: usize) -> impl FnMut(&'a str) -> IResult<&str, u8, E>
where
    E: ParseError<&'a str> + FromExternalError<&'a str, ParseIntError>,
{
    map_res(
        nom::bytes::complete::take_while_m_n(digits, digits, |c: char| c.is_digit(10)),
        |s: &str| s.parse(),
    )
}

fn u32_complete<'a, E>(digits: usize) -> impl FnMut(&'a str) -> IResult<&str, u32, E>
where
    E: ParseError<&'a str> + FromExternalError<&'a str, ParseIntError>,
{
    map_res(
        nom::bytes::complete::take_while_m_n(digits, digits, |c: char| c.is_digit(10)),
        |s: &str| s.parse(),
    )
}

fn fixed_point<'a, E>(
    digits: usize,
    decimals: usize,
) -> impl FnMut(&'a str) -> IResult<&str, u32, E>
where
    E: ParseError<&'a str> + FromExternalError<&'a str, ParseIntError>,
{
    let integer = map_res(
        terminated(
            take_while_m_n(digits, digits, |c: char| c.is_digit(10)),
            tag("."),
        ),
        |s: &str| s.parse(),
    );
    let fractional = map_res(
        take_while_m_n(decimals, decimals, |c: char| c.is_digit(10)),
        |s: &str| s.parse(),
    );
    map_res(integer.and(fractional), move |res: (u32, u32)| {
        Ok(res.0 * 10u32.pow(decimals as u32) + res.1)
    })
}

fn decode_hex<'a>(data: &'a str, out: &mut [u8]) -> Result<(), nom::error::Error<&'a str>> {
    fn hex_val(c: u8, idx: usize) -> Option<u8> {
        match c {
            b'A'..=b'F' => Some(c - b'A' + 10),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'0'..=b'9' => Some(c - b'0'),
            _ => None,
        }
    }

    let err = || nom::error::Error {
        input: data,
        code: nom::error::ErrorKind::HexDigit,
    };
    let data = data.as_bytes();
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = hex_val(data[2 * i], 2 * i).ok_or_else(err)? << 4
            | hex_val(data[2 * i + 1], 2 * i + 1).ok_or_else(err)?;
    }

    Ok(())
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc >>= 1;
                crc ^= 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
#[macro_use]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use nom::{error::ErrorKind, multi::fill, Err};
    use std::string::String;
    type TestResult<'a, O> = IResult<&'a str, O, nom::error::Error<&'a str>>;

    const EXAMPLE_TELEGRAM: &[u8] = b"/XMX5LGBBFFB231237741\r\n\r\n\
    1-3:0.2.8(42)\r\n\
    0-0:1.0.0(200208153516W)\r\n\
    0-0:96.1.1(4530303034303031383434303034323134)\r\n\
    1-0:1.8.1(004436.791*kWh)\r\n\
    1-0:2.8.1(000000.000*kWh)\r\n\
    1-0:1.8.2(004234.483*kWh)\r\n\
    1-0:2.8.2(000000.000*kWh)\r\n\
    0-0:96.14.0(0001)\r\n\
    1-0:1.7.0(00.329*kW)\r\n\
    1-0:2.7.0(00.000*kW)\r\n\
    0-0:96.7.21(00002)\r\n\
    0-0:96.7.9(00003)\r\n\
    1-0:99.97.0(3)(0-0:96.7.19)(180726223917S)(0000006462*s)(170325035658W)(0036416374*s)(160128161754W)(0024464269*s)\r\n\
    1-0:32.32.0(00000)\r\n\
    1-0:32.36.0(00000)\r\n\
    0-0:96.13.1()\r\n\
    0-0:96.13.0()\r\n\
    1-0:31.7.0(002*A)\r\n\
    1-0:21.7.0(00.329*kW)\r\n\
    1-0:22.7.0(00.000*kW)\r\n\
    !6130\r\n";

    const TWO_TELEGRAMS: &[u8] = b"/XMX5LGBBFFB231237741\r\n\r\n\
    1-3:0.2.8(42)\r\n\
    0-0:1.0.0(200208153516W)\r\n\
    0-0:96.1.1(4530303034303031383434303034323134)\r\n\
    1-0:1.8.1(004436.791*kWh)\r\n\
    1-0:2.8.1(000000.000*kWh)\r\n\
    1-0:1.8.2(004234.483*kWh)\r\n\
    1-0:2.8.2(000000.000*kWh)\r\n\
    0-0:96.14.0(0001)\r\n\
    1-0:1.7.0(00.329*kW)\r\n\
    1-0:2.7.0(00.000*kW)\r\n\
    0-0:96.7.21(00002)\r\n\
    0-0:96.7.9(00003)\r\n\
    1-0:99.97.0(3)(0-0:96.7.19)(180726223917S)(0000006462*s)(170325035658W)(0036416374*s)(160128161754W)(0024464269*s)\r\n\
    1-0:32.32.0(00000)\r\n\
    1-0:32.36.0(00000)\r\n\
    0-0:96.13.1()\r\n\
    0-0:96.13.0()\r\n\
    1-0:31.7.0(002*A)\r\n\
    1-0:21.7.0(00.329*kW)\r\n\
    1-0:22.7.0(00.000*kW)\r\n\
    !6130\r\n/XMX5LGBBFFB231237741\r\n\r\n\
    1-3:0.2.8(42)\r\n\
    0-0:1.0.0(200208153516W)\r\n\
    0-0:96.1.1(4530303034303031383434303034323134)\r\n\
    1-0:1.8.1(004436.791*kWh)\r\n\
    1-0:2.8.1(000000.000*kWh)\r\n\
    1-0:1.8.2(004234.483*kWh)\r\n\
    1-0:2.8.2(000000.000*kWh)\r\n\
    0-0:96.14.0(0001)\r\n\
    1-0:1.7.0(00.329*kW)\r\n\
    1-0:2.7.0(00.000*kW)\r\n\
    0-0:96.7.21(00002)\r\n\
    0-0:96.7.9(00003)\r\n\
    1-0:99.97.0(3)(0-0:96.7.19)(180726223917S)(0000006462*s)(170325035658W)(0036416374*s)(160128161754W)(0024464269*s)\r\n\
    1-0:32.32.0(00000)\r\n\
    1-0:32.36.0(00000)\r\n\
    0-0:96.13.1()\r\n\
    0-0:96.13.0()\r\n\
    1-0:31.7.0(002*A)\r\n\
    1-0:21.7.0(00.329*kW)\r\n\
    1-0:22.7.0(00.000*kW)\r\n\
    !6130\r\n";

    #[test]
    fn test_serialize() {
        let (read, res) = parse(EXAMPLE_TELEGRAM);
        let res = res.unwrap();
        let mut s = String::new();
        res.serialize(&mut s);
        println!("{}", s);
    }

    #[test]
    fn telegram_parses() {
        let (read, res) = parse(EXAMPLE_TELEGRAM);
        let res = res.unwrap();
        assert_eq!(EXAMPLE_TELEGRAM.len(), read);
        println!("{:?}", res);
    }

    #[test]
    fn two_telegrams_parse_successively() {
        let (read1, res) = parse(TWO_TELEGRAMS);
        let telegram1 = res.unwrap();
        let (read2, res) = parse(&TWO_TELEGRAMS[read1..]);
        let telegram2 = res.unwrap();

        assert_eq!(TWO_TELEGRAMS.len(), read1 + read2);
    }

    #[test]
    fn incomplete_telegram_err_incomplete() {
        for length in 0..EXAMPLE_TELEGRAM.len() {
            let (read, res) = parse(&EXAMPLE_TELEGRAM[..length]);
            match res {
                Err(TelegramParseError::Incomplete) => {}
                other => {
                    panic!("Expected incomplete but got {:?}", other);
                }
            }
            assert_eq!(0, read);
        }
    }

    #[test]
    fn simple_telegram_parses() {
        let mut line_buffer = ArrayVec::<_, 32>::new();
        let res: TestResult<Telegram> = telegram(
            "/XMX1000\r\n\r\n1-3:0.2.8(42)\r\n0-0:1.0.0(200208153506W)\r\n!FFFF\r\n",
            line_buffer,
        );
        let (rem, tel) = res.unwrap();
        assert_eq!("XMX1000", tel.device_id.as_str());
        assert_eq!(2, tel.lines.len());
        assert_eq!(65535, tel.crc);
    }

    #[test]
    fn single_value_line_parses() {
        let res: TestResult<Line> = line("1-3:0.2.8(42)\r\n");
        let (rem, line) = res.unwrap();
        match line {
            Line::Version(ver) => assert_eq!(42, ver),
            var => panic!("Unexpected enum variant: {:?}", var),
        }
    }

    #[test]
    fn single_value_raw_line_parses() {
        let res: TestResult<RawLine> = raw_line("0-0:96.14.0(0002)\r\n");
        let (rem, line) = res.unwrap();
        assert_eq!([0, 0, 96, 14, 0, 255], line.obis);
        assert_eq!("0002", line.cosem[0]);
        assert_eq!(1, line.cosem.len());
        assert_eq!("", rem);
    }

    #[test]
    fn multiple_value_raw_line_parses() {
        let res: TestResult<RawLine> = raw_line("0-1:24.2.1(101209110000W)(12785.123*m3)\r\n");
        let (rem, line) = res.unwrap();
        assert_eq!([0, 1, 24, 2, 1, 255], line.obis);
        assert_eq!("101209110000W", line.cosem[0]);
        assert_eq!("12785.123*m3", line.cosem[1]);
        assert_eq!(2, line.cosem.len());
        assert_eq!("", rem);
    }

    #[test]
    fn invalid_cosem_fails() {
        let res: TestResult<&str> = cosem()("invalid string");
        match res.unwrap_err() {
            Err::Error(t) => {}
            _ => panic!("Expected parse error"),
        }
    }

    #[test]
    fn valid_cosem_parses() {
        let res: TestResult<&str> = cosem()("(00.000*kW)");
        let (_, cosem) = res.unwrap();
        assert_eq!("00.000*kW", cosem)
    }

    #[test]
    fn obis_without_tag_f_parses() {
        let res: TestResult<[u8; 6]> = obis_code("0-0:96.7.21()");
        let (rem, obis) = res.unwrap();
        assert_eq!("()", rem);
        assert_eq!([0, 0, 96, 7, 21, 255], obis)
    }

    #[test]
    fn obis_with_tag_f_parses() {
        let res: TestResult<[u8; 6]> = obis_code("255-255:0.1.0.18()");
        let (rem, obis) = res.unwrap();
        assert_eq!("()", rem);
        assert_eq!([255, 255, 0, 1, 0, 18], obis)
    }

    #[test]
    fn u8_complete_parses() {
        let res: TestResult<u8> = u8_complete(2)("38");
        let (rem, val) = res.unwrap();
        assert_eq!(38, val);
    }

    #[test]
    fn crc_parses() {
        let res: TestResult<u16> = crc("!FE01\r\n");
        let (rem, crc) = res.unwrap();
        assert_eq!(65025, crc);
    }

    #[test]
    fn crc16_matches() {
        let data = b"123456789";
        let crc = crc16(data);
        assert_eq!(0xbb3d, crc);
    }

    #[test]
    fn crc16_full_telegram_matches() {
        // CRC (4 bytes) and final CRLF (2 bytes)
        const TRAILER: usize = 6;
        let crc = crc16(&EXAMPLE_TELEGRAM[..EXAMPLE_TELEGRAM.len() - TRAILER]);
        assert_eq!(0x6130, crc);
    }
}

// One-shot test: write a URL into the URL area and bind a key to it.
// Command 0x40 gives no reply on this model, so whether the write lands is
// exactly what the operator is about to find out by pressing the key.
use minikbconfig_hid::Pad;

fn main() {
    let pad = Pad::open().expect("open pad");
    let text = std::env::args().nth(1).unwrap_or_default();
    let index: u8 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "0".into())
        .parse()
        .expect("index");

    let encoded = encode(&text);
    println!("{text:?} -> {} bytes: {}", encoded.len(), hex(&encoded));

    // 0xFF terminates, so pad the rest of the 128-byte area with it.
    let mut area = encoded.clone();
    area.resize(128, 0xFF);

    let mut offset = 0usize;
    while offset < area.len() {
        let chunk = &area[offset..(offset + 59).min(area.len())];
        let mut r = [0u8; 64];
        r[0] = 0x06;
        r[1] = 0x40;
        r[2] = chunk.len() as u8;
        r[3] = (offset & 0xFF) as u8;
        r[4] = (offset >> 8) as u8;
        // Payload at byte 5, like the macro and light records: 5 + 59 = 64,
        // which is why the vendor chunks writes at 59.
        r[5..5 + chunk.len()].copy_from_slice(chunk);
        match pad.raw_exchange(&r, 0x40) {
            Ok(reply) => println!("  0x40 offset {offset}: reply {:02x?}", &reply[..6]),
            Err(e) => println!("  0x40 offset {offset}: no reply ({e})"),
        }
        offset += 59;
    }

    pad.write_key(index, minikbconfig_core::KeyType::OpenWebsite, 0)
        .expect("bind key");
    println!(
        "key[{index}] bound to OpenWebsite, stored back: {:?}",
        pad.read_key(index, 0).unwrap()
    );
    println!(
        "\nPress physical key {} and see whether a browser opens.",
        index + 1
    );
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The vendor's table. Digits and their punctuation twins share a byte, which
/// is a read-side defect - the write side is unambiguous.
fn encode(text: &str) -> Vec<u8> {
    text.chars().filter_map(value_of).collect()
}

fn value_of(c: char) -> Option<u8> {
    Some(match c {
        'a' | 'A' => 4,
        'b' | 'B' => 5,
        'c' | 'C' => 6,
        'd' | 'D' => 7,
        'e' | 'E' => 8,
        'f' | 'F' => 9,
        'g' | 'G' => 10,
        'h' | 'H' => 11,
        'i' | 'I' => 12,
        'j' | 'J' => 13,
        'k' | 'K' => 14,
        'l' | 'L' => 15,
        'm' | 'M' => 16,
        'n' | 'N' => 17,
        'o' | 'O' => 18,
        'p' | 'P' => 19,
        'q' | 'Q' => 20,
        'r' | 'R' => 21,
        's' | 'S' => 22,
        't' | 'T' => 23,
        'u' | 'U' => 24,
        'v' | 'V' => 25,
        'w' | 'W' => 26,
        'x' | 'X' => 27,
        'y' | 'Y' => 28,
        'z' | 'Z' => 29,
        '1' => 30,
        '2' => 31,
        '3' => 32,
        '4' => 33,
        '5' => 34,
        '6' => 35,
        '7' => 36,
        '8' => 37,
        '9' => 38,
        '0' => 39,
        ' ' => 44,
        '-' | '_' => 45,
        '=' => 46,
        '[' => 47,
        ']' => 48,
        '\\' => 50,
        ';' | ':' => 51,
        '\'' => 52,
        '`' => 53,
        ',' => 54,
        '.' | '>' => 55,
        '/' | '?' => 56,
        _ => return None,
    })
}

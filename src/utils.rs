use crate::state::Location;

pub fn fuck_socketio(mut msg: String) -> Option<String> {
    // forward msg until first non-digit char
    while msg.chars().next().unwrap().is_ascii_digit() {
        msg = msg.chars().skip(1).collect();
        if msg.len() == 0 {
            return None;
        }
    }

    Some(msg)
}

pub fn int_to_location(
    mut i: u64,
    width: u64,
    height: u64,
    pad_left: u64,
    pad_top: u64,
) -> Location {
    if i > width * height {
        i = i % (width * height);
    }
    let x: usize = (i % width).try_into().unwrap();
    (
        x + pad_left as usize,
        // floor div
        ((i as usize - x) / width as usize) + pad_top as usize,
    )
}

pub fn location_to_int(
    mut location: Location,
    width: u64,
    height: u64,
    pad_left: u64,
    pad_top: u64,
) -> u64 {
    location.0 -= pad_left as usize;
    location.1 -= pad_top as usize;

    location.0 as u64 + location.1 as u64 * width
}

#[inline]
pub fn get_neighbors(location: Location, width: usize, height: usize) -> Vec<Location> {
    // 4 results with bound checking
    let mut neighbors = vec![];
    if location.0 > 0 {
        neighbors.push((location.0 - 1, location.1));
    }
    if location.0 < width as usize - 1 {
        neighbors.push((location.0 + 1, location.1));
    }
    if location.1 > 0 {
        neighbors.push((location.0, location.1 - 1));
    }
    if location.1 < height as usize - 1 {
        neighbors.push((location.0, location.1 + 1));
    }
    neighbors
}

pub fn manhattan_distance(a: Location, b: Location) -> u64 {
    ((a.0 as i64 - b.0 as i64).abs() + (a.1 as i64 - b.1 as i64).abs()) as u64
}

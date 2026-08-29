// MIT License
//
// Copyright (c) 2026 vanilla-aruco contributors

//! The computation-only backend for vanilla-aruco.
//!
//! The plugin deliberately returns path *segments*, rather than SVG or PDF.
//! The Typst wrapper owns physical sizing and turns the segments into a
//! `curve`, which keeps the WASM interface small and the output vector-native.

#[cfg(target_arch = "wasm32")]
use wasm_minimal_protocol::*;

use serde::{Deserialize, Serialize};

mod dictionaries;

#[cfg(target_arch = "wasm32")]
initiate_protocol!();

const VERSION: u8 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct Request {
    version: u8,
    dictionary: String,
    id: u16,
    turns: u8,
}

#[derive(Debug, Deserialize, Serialize)]
struct Response {
    version: u8,
    segments: Vec<WireSegment>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WireSegment {
    kind: u8,
    dx: i16,
    dy: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
    Right,
    Left,
}

impl Direction {
    fn flip(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }
}

/// An oriented position on one edge of the cell grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    row: i16,
    col: i16,
    dir: Direction,
}

impl Position {
    fn end_node(self) -> (i16, i16) {
        match self.dir {
            Direction::Up | Direction::Left => (self.row, self.col),
            Direction::Down => (self.row + 1, self.col),
            Direction::Right => (self.row, self.col + 1),
        }
    }

    fn start_node(self) -> (i16, i16) {
        Self {
            dir: self.dir.flip(),
            ..self
        }
        .end_node()
    }

    fn straight(self) -> Self {
        let (row, col) = match self.dir {
            Direction::Up => (self.row - 1, self.col),
            Direction::Down => (self.row + 1, self.col),
            Direction::Right => (self.row, self.col + 1),
            Direction::Left => (self.row, self.col - 1),
        };
        Self { row, col, ..self }
    }

    fn left(self) -> Self {
        let (row, col, dir) = match self.dir {
            Direction::Up => (self.row, self.col - 1, Direction::Left),
            Direction::Down => (self.row + 1, self.col, Direction::Right),
            Direction::Right => (self.row - 1, self.col + 1, Direction::Up),
            Direction::Left => (self.row, self.col, Direction::Down),
        };
        Self { row, col, dir }
    }

    fn right(self) -> Self {
        let (row, col, dir) = match self.dir {
            Direction::Up => (self.row, self.col, Direction::Right),
            Direction::Down => (self.row + 1, self.col - 1, Direction::Left),
            Direction::Right => (self.row, self.col + 1, Direction::Down),
            Direction::Left => (self.row - 1, self.col, Direction::Up),
        };
        Self { row, col, dir }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Segment {
    Move(i16, i16),
    Horizontal(i16),
    Vertical(i16),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MicroStep {
    Jump((i16, i16)),
    Step((i16, i16)),
}

#[derive(Debug)]
struct Graph {
    left: Vec<bool>,
    top: Vec<bool>,
    width: i16,
    height: i16,
    edge_hint: usize,
}

impl Graph {
    fn index(&self, row: i16, col: i16) -> usize {
        row as usize * (self.width as usize + 1) + col as usize
    }

    fn has_cell(&self, row: i16, col: i16) -> bool {
        row >= 0 && row <= self.height && col >= 0 && col <= self.width
    }

    fn has_edge(&self, pos: Position) -> bool {
        if !self.has_cell(pos.row, pos.col) {
            return false;
        }
        let index = self.index(pos.row, pos.col);
        match pos.dir {
            Direction::Left | Direction::Right => self.top[index],
            Direction::Up | Direction::Down => self.left[index],
        }
    }

    fn remove_edge(&mut self, pos: Position) {
        let index = self.index(pos.row, pos.col);
        match pos.dir {
            Direction::Left | Direction::Right => self.top[index] = false,
            Direction::Up | Direction::Down => self.left[index] = false,
        }
    }

    fn can_step(&self, pos: Position) -> Option<Position> {
        [pos.straight(), pos.left(), pos.right()]
            .into_iter()
            .find(|candidate| self.has_edge(*candidate))
    }

    fn follow(&self, pos: Position) -> (Option<Position>, bool) {
        let mut found = None;
        let mut alternatives = false;
        for candidate in [pos.straight(), pos.left(), pos.right()] {
            if self.has_edge(candidate) {
                if found.is_none() {
                    found = Some(candidate);
                } else {
                    alternatives = true;
                }
            }
        }
        (found, alternatives)
    }

    fn edge_left(&mut self) -> Option<Position> {
        for index in self.edge_hint..self.left.len() {
            if self.left[index] || self.top[index] {
                self.edge_hint = index;
                let row = (index / (self.width as usize + 1)) as i16;
                let col = (index % (self.width as usize + 1)) as i16;
                return Some(Position {
                    row,
                    col,
                    dir: if self.top[index] {
                        Direction::Right
                    } else {
                        Direction::Up
                    },
                });
            }
        }
        self.edge_hint = self.left.len();
        None
    }
}

fn bit_count(size: u8) -> Result<usize, String> {
    let size = size as usize;
    if size == 0 || size > 250 {
        return Err("vanilla-aruco: marker size must be between 1 and 250".into());
    }
    Ok(size * size)
}

fn decode_word(size: u8, bytes: &[u8], turns: u8) -> Result<Vec<bool>, String> {
    let required = bit_count(size)?;
    if turns > 3 {
        return Err("vanilla-aruco: rotation must be between 0 and 3 quarter-turns".into());
    }
    if bytes.len() * 8 < required {
        return Err("vanilla-aruco: dictionary word does not contain enough bits".into());
    }

    let mut bits = Vec::with_capacity(required);
    for byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte & (1 << shift)) != 0);
        }
    }
    bits.truncate(required);

    let size = size as usize;
    for _ in 0..turns {
        let mut rotated = vec![false; required];
        for row in 0..size {
            for col in 0..size {
                rotated[row * size + col] = bits[(size - 1 - col) * size + row];
            }
        }
        bits = rotated;
    }
    Ok(bits)
}

fn make_marker(data: &[bool], size: u8) -> (Vec<bool>, usize, usize) {
    let data_size = size as usize;
    let width = data_size + 2;
    let mut marker = vec![false; width * width];
    for row in 0..width {
        for col in 0..width {
            marker[row * width + col] = row == 0
                || row == width - 1
                || col == 0
                || col == width - 1
                || (row > 0
                    && row < width - 1
                    && col > 0
                    && col < width - 1
                    && data[(row - 1) * data_size + col - 1]);
        }
    }
    (marker, width, width)
}

fn build_graph(bits: &[bool], width: usize, height: usize) -> Graph {
    let edge_count = (width + 1) * (height + 1);
    let mut graph = Graph {
        left: vec![false; edge_count],
        top: vec![false; edge_count],
        width: width as i16,
        height: height as i16,
        edge_hint: edge_count,
    };
    let mut hint = None;

    for row in 0..height {
        for col in 0..width {
            let index = row * width + col;
            if !bits[index] {
                continue;
            }
            let cell = row * (width + 1) + col;
            hint.get_or_insert(cell);
            if col == 0 || !bits[index - 1] {
                graph.left[cell] = true;
            }
            if row == 0 || !bits[index - width] {
                graph.top[cell] = true;
            }
            if col == width - 1 || !bits[index + 1] {
                graph.left[cell + 1] = true;
            }
            if row == height - 1 || !bits[index + width] {
                graph.top[cell + width + 1] = true;
            }
        }
    }
    graph.edge_hint = hint.unwrap_or(edge_count);
    graph
}

fn euler_steps(bits: &[bool], width: usize, height: usize) -> Vec<MicroStep> {
    let mut graph = build_graph(bits, width, height);
    let mut pos = match graph.edge_left() {
        Some(pos) => pos,
        None => return Vec::new(),
    };
    let mut elements = Vec::new();
    let mut alternatives: Vec<(usize, Position)> = Vec::new();
    let mut insert = 0usize;

    loop {
        'euler: loop {
            let mut local_loop = Vec::new();
            let insert_pos = insert;

            graph.remove_edge(pos);
            let start = pos.start_node();
            local_loop.push(MicroStep::Step(pos.end_node()));
            insert += 1;

            loop {
                let (new_pos, had_alternatives) = graph.follow(pos);
                if had_alternatives {
                    alternatives.push((insert, pos));
                }
                let next = new_pos.expect("boundary graph must have a continuation");
                pos = next;
                graph.remove_edge(pos);
                let end = pos.end_node();
                local_loop.push(MicroStep::Step(end));
                if end == start {
                    break;
                }
                insert += 1;
            }

            elements.splice(insert_pos..insert_pos, local_loop);

            for (index, alternative) in alternatives.drain(..) {
                if let Some(next) = graph.can_step(alternative) {
                    pos = next;
                    insert = index;
                    continue 'euler;
                }
            }
            break;
        }

        let Some(next) = graph.edge_left() else {
            break;
        };
        elements.push(MicroStep::Jump(next.start_node()));
        pos = next;
        insert = elements.len();
    }
    elements
}

fn compress_path(steps: &[MicroStep]) -> Vec<Segment> {
    #[derive(Clone, Copy)]
    enum Work {
        Horizontal(i16),
        Vertical(i16),
    }

    let mut segments = Vec::new();
    let mut pos = (0i16, 0i16);
    let mut work = None;

    let flush = |segments: &mut Vec<Segment>, work: &mut Option<Work>| {
        if let Some(value) = work.take() {
            match value {
                Work::Horizontal(length) => segments.push(Segment::Horizontal(length)),
                Work::Vertical(length) => segments.push(Segment::Vertical(length)),
            }
        }
    };

    for step in steps {
        match *step {
            MicroStep::Step((row, col)) => {
                match work {
                    Some(Work::Horizontal(length)) if row == pos.0 => {
                        work = Some(Work::Horizontal(length + col - pos.1));
                    }
                    Some(Work::Vertical(length)) if col == pos.1 => {
                        work = Some(Work::Vertical(length + row - pos.0));
                    }
                    _ => {
                        flush(&mut segments, &mut work);
                        work = if row == pos.0 {
                            Some(Work::Horizontal(col - pos.1))
                        } else {
                            Some(Work::Vertical(row - pos.0))
                        };
                    }
                }
                pos = (row, col);
            }
            MicroStep::Jump((row, col)) => {
                work = None;
                segments.push(Segment::Close);
                segments.push(Segment::Move(col - pos.1, row - pos.0));
                pos = (row, col);
            }
        }
    }
    flush(&mut segments, &mut work);
    if !segments.is_empty() {
        segments.push(Segment::Close);
    }
    segments
}

fn predefined_word(name: &str, id: u16) -> Result<(u8, &'static [u8]), String> {
    let id = id as usize;
    let result = match name {
        "DICT_ARUCO_ORIGINAL" if id < dictionaries::ARUCO_ORIGINAL.len() => {
            Some((5, dictionaries::ARUCO_ORIGINAL[id].as_slice()))
        }
        "DICT_4X4_50" if id < 50 => Some((4, dictionaries::ARUCO_4X4[id].as_slice())),
        "DICT_4X4_100" if id < 100 => Some((4, dictionaries::ARUCO_4X4[id].as_slice())),
        "DICT_4X4_250" if id < 250 => Some((4, dictionaries::ARUCO_4X4[id].as_slice())),
        "DICT_4X4_1000" if id < dictionaries::ARUCO_4X4.len() => {
            Some((4, dictionaries::ARUCO_4X4[id].as_slice()))
        }
        "DICT_5X5_50" if id < 50 => Some((5, dictionaries::ARUCO_5X5[id].as_slice())),
        "DICT_5X5_100" if id < 100 => Some((5, dictionaries::ARUCO_5X5[id].as_slice())),
        "DICT_5X5_250" if id < 250 => Some((5, dictionaries::ARUCO_5X5[id].as_slice())),
        "DICT_5X5_1000" if id < dictionaries::ARUCO_5X5.len() => {
            Some((5, dictionaries::ARUCO_5X5[id].as_slice()))
        }
        "DICT_6X6_50" if id < 50 => Some((6, dictionaries::ARUCO_6X6[id].as_slice())),
        "DICT_6X6_100" if id < 100 => Some((6, dictionaries::ARUCO_6X6[id].as_slice())),
        "DICT_6X6_250" if id < 250 => Some((6, dictionaries::ARUCO_6X6[id].as_slice())),
        "DICT_6X6_1000" if id < dictionaries::ARUCO_6X6.len() => {
            Some((6, dictionaries::ARUCO_6X6[id].as_slice()))
        }
        "DICT_7X7_50" if id < 50 => Some((7, dictionaries::ARUCO_7X7[id].as_slice())),
        "DICT_7X7_100" if id < 100 => Some((7, dictionaries::ARUCO_7X7[id].as_slice())),
        "DICT_7X7_250" if id < 250 => Some((7, dictionaries::ARUCO_7X7[id].as_slice())),
        "DICT_7X7_1000" if id < dictionaries::ARUCO_7X7.len() => {
            Some((7, dictionaries::ARUCO_7X7[id].as_slice()))
        }
        "DICT_ARUCO_MIP_36h12" if id < dictionaries::ARUCO_MIP_36H12.len() => {
            Some((6, dictionaries::ARUCO_MIP_36H12[id].as_slice()))
        }
        _ => None,
    };
    result.ok_or_else(|| {
        format!("vanilla-aruco: unknown dictionary or invalid marker id: {name} {id}")
    })
}

fn encode_segments(segments: &[Segment]) -> Result<Vec<u8>, String> {
    let wire = segments
        .iter()
        .map(|segment| match *segment {
            Segment::Move(dx, dy) => WireSegment { kind: 0, dx, dy },
            Segment::Horizontal(length) => WireSegment {
                kind: 1,
                dx: length,
                dy: 0,
            },
            Segment::Vertical(length) => WireSegment {
                kind: 2,
                dx: 0,
                dy: length,
            },
            Segment::Close => WireSegment {
                kind: 3,
                dx: 0,
                dy: 0,
            },
        })
        .collect();
    let response = Response {
        version: VERSION,
        segments: wire,
    };
    let mut output = Vec::new();
    ciborium::into_writer(&response, &mut output)
        .map_err(|error| format!("vanilla-aruco: failed to encode CBOR response: {error}"))?;
    Ok(output)
}

fn generate(input: &[u8]) -> Result<Vec<u8>, String> {
    let request: Request = ciborium::from_reader(input)
        .map_err(|error| format!("vanilla-aruco: invalid CBOR request: {error}"))?;
    if request.version != VERSION {
        return Err("vanilla-aruco: unsupported CBOR request version".into());
    }
    let (size, word) = predefined_word(&request.dictionary, request.id)?;
    let bits = decode_word(size, word, request.turns)?;
    let (marker, width, height) = make_marker(&bits, size);
    let steps = euler_steps(&marker, width, height);
    encode_segments(&compress_path(&steps))
}

/// Generate optimized boundary segments for one ArUco codeword.
#[cfg_attr(target_arch = "wasm32", wasm_func)]
pub fn generate_path(input: &[u8]) -> Result<Vec<u8>, String> {
    generate(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(dictionary: &str, id: u16, turns: u8) -> Vec<u8> {
        let request = Request {
            version: VERSION,
            dictionary: dictionary.into(),
            id,
            turns,
        };
        let mut input = Vec::new();
        ciborium::into_writer(&request, &mut input).unwrap();
        input
    }

    #[test]
    fn opencv_word_is_read_msb_first() {
        let bits = decode_word(4, &[181, 50], 0).unwrap();
        assert_eq!(
            bits,
            [
                true, false, true, true, false, true, false, true, false, false, true, true, false,
                false, true, false,
            ]
        );
    }

    #[test]
    fn rotation_is_clockwise() {
        let bits = decode_word(2, &[0b1001_0000], 1).unwrap();
        assert_eq!(bits, [false, true, true, false]);
    }

    #[test]
    fn simple_square_has_one_closed_path() {
        let steps = euler_steps(&[true], 1, 1);
        assert_eq!(
            compress_path(&steps),
            vec![
                Segment::Horizontal(1),
                Segment::Vertical(1),
                Segment::Horizontal(-1),
                Segment::Vertical(-1),
                Segment::Close,
            ]
        );
    }

    #[test]
    fn euler_mini_2x2_one_component() {
        let steps = euler_steps(&[true, false, true, true], 2, 2);
        assert_eq!(
            compress_path(&steps),
            vec![
                Segment::Horizontal(1),
                Segment::Vertical(1),
                Segment::Horizontal(1),
                Segment::Vertical(1),
                Segment::Horizontal(-2),
                Segment::Vertical(-2),
                Segment::Close,
            ]
        );
    }

    #[test]
    fn euler_mini_2x3_one_component() {
        let steps = euler_steps(&[true, false, true, true, true, false], 3, 2);
        assert_eq!(
            compress_path(&steps),
            vec![
                Segment::Horizontal(1),
                Segment::Vertical(1),
                Segment::Horizontal(2),
                Segment::Vertical(-1),
                Segment::Horizontal(-1),
                Segment::Vertical(2),
                Segment::Horizontal(-2),
                Segment::Vertical(-2),
                Segment::Close,
            ]
        );
    }

    #[test]
    fn euler_mini_3x2_two_components() {
        let steps = euler_steps(&[true, true, false, false, false, true], 2, 3);
        assert_eq!(
            compress_path(&steps),
            vec![
                Segment::Horizontal(2),
                Segment::Vertical(1),
                Segment::Horizontal(-2),
                Segment::Close,
                Segment::Move(1, 2),
                Segment::Horizontal(1),
                Segment::Vertical(1),
                Segment::Horizontal(-1),
                Segment::Vertical(-1),
                Segment::Close,
            ]
        );
    }

    #[test]
    fn empty_bitmap_has_no_path() {
        let steps = euler_steps(&[false; 6], 2, 3);
        assert!(compress_path(&steps).is_empty());
    }

    #[test]
    fn plugin_wire_format_round_trips_response() {
        let output = generate_path(&request("DICT_4X4_50", 0, 0)).unwrap();
        let response: Response = ciborium::from_reader(output.as_slice()).unwrap();
        assert_eq!(response.version, VERSION);
        assert!(!response.segments.is_empty());
    }

    #[test]
    fn predefined_dictionaries_cover_common_sizes() {
        assert_eq!(predefined_word("DICT_4X4_50", 49).unwrap().0, 4);
        assert_eq!(predefined_word("DICT_5X5_100", 99).unwrap().0, 5);
        assert_eq!(predefined_word("DICT_6X6_250", 249).unwrap().0, 6);
        assert_eq!(predefined_word("DICT_7X7_1000", 999).unwrap().0, 7);
        assert_eq!(predefined_word("DICT_ARUCO_ORIGINAL", 1023).unwrap().0, 5);
        assert_eq!(predefined_word("DICT_ARUCO_MIP_36h12", 249).unwrap().0, 6);
        assert!(predefined_word("DICT_4X4_50", 50).is_err());
    }
}


use leaprs::*;
use midir::{MidiOutput, MidiOutputConnection};
use mouse_rs::Mouse;
use std::{
    collections::HashMap,
    error::Error,
    time::{Duration, Instant},
};
use crossterm::{
    execute,
    terminal::{Clear, ClearType},
};

const SCREEN_HEIGHT: i32 = 1020;
const SCREEN_WIDTH: i32 = 1920;

// leap motion controller säätöjä
const MAX_X: f32 = 300.0;
const MIN_X: f32 = -300.0;

const MAX_Y: f32 = 220.0;
const MIN_Y: f32 = 500.0;

const MAX_Z: f32 = 0.0;
const MIN_Z: f32 = -100.0;

const MIDI_CHANNEL: u8 = 2;
const MIDI_DELAY_MS: u64 = 1000;
const MOVING_AVERAGE_SAMPLES: usize = 3;
//const MOVEMENT_THRESHOLD: i32 = 300;

const BASE_NOTE: u8 = 42; // MIDI note value for F#2
const OCTAVE_SIZE: u8 = 12;

fn generate_minor_scale(octaves: u8) -> Vec<u8> {
    let mut scale = Vec::new();
    for octave in 0..octaves {
        for note in &[0, 2, 3, 5, 7, 9, 10] {
            scale.push(BASE_NOTE + octave * OCTAVE_SIZE + note);
        }
    }
    scale
}
struct MovingAverage {
    samples: Vec<(i32, i32, i32)>,
}

impl MovingAverage {
    fn new() -> Self {
        MovingAverage {
            samples: vec![(0, 0, 0); MOVING_AVERAGE_SAMPLES],
        }
    }

    fn add_sample(&mut self, x: i32, y: i32, z: i32) {
        self.samples.push((x, y, z));
        if self.samples.len() > MOVING_AVERAGE_SAMPLES {
            self.samples.remove(0);
        }
    }

    fn get_smoothed_position(&self) -> (i32, i32, i32) {
        let sum_x: i32 = self.samples.iter().map(|&(x, _, _)| x).sum();
        let sum_y: i32 = self.samples.iter().map(|&(_, y, _)| y).sum();
        let sum_z: i32 = self.samples.iter().map(|&(_, _, z)| z).sum();
        let count = self.samples.len() as i32;
        (sum_x / count, sum_y / count, sum_z / count)
    }
}

fn hand_tracking(hand: &Hand<'_>, moving_average: &mut MovingAverage) -> (i32, i32, i32) {
    let leap_x = hand.palm().position().x();
    let leap_y = hand.palm().position().y();
    let leap_z = hand.palm().position().z();
    // Check if the hand is within a valid range
    if leap_x.is_nan() || leap_y.is_nan() || leap_z.is_nan() {
        // Hand data is invalid, return None
        return (0, 0, 0);
    }
    moving_average.add_sample(leap_x as i32, leap_y as i32, leap_z as i32);
    let (smoothed_x, smoothed_y, smoothed_z) = moving_average.get_smoothed_position();
    //println!(" x{:?} y{:?} z{:?}", smoothed_x, smoothed_y, screen_z);
    (smoothed_x, smoothed_y, smoothed_z)
}

fn map_leap_coordinates_to_screen(leap_x: f32, leap_y: f32) -> (i32, i32) {
    let screen_x = ((leap_x - MIN_X) / (MAX_X - MIN_X)) * SCREEN_WIDTH as f32;
    let screen_y = ((leap_y - MIN_Y) / (MAX_Y - MIN_Y)) * SCREEN_HEIGHT as f32;
    //let screen_z = ((leap_z - MIN_Z) / (MAX_Z - MIN_Z)) * 3000 as f32;

    //println!("LeapX: {} LeapY: {} SX: {} SY: {} ", leap_x as i32, leap_y as i32, screen_x as i32, screen_y as i32);

    (screen_x as i32, screen_y as i32)
}

fn find_nearest_note_in_scale(pitch: u8, scale: &Vec<u8>) -> u8 {
    let mut nearest_note = scale[0];
    let mut min_distance = u8::MAX;

    for &note in scale.iter() {
        let distance = (pitch as i16 - note as i16).abs() as u8;
        if distance < min_distance {
            min_distance = distance;
            nearest_note = note;
        }
    }
    nearest_note
}

fn map_to_midi(value: f32, leap_min: f32, leap_max: f32, midi_range: f32) -> u8 {
    let leap_range = leap_max - leap_min;

    // Calculate the scale factor
    let scale = midi_range / leap_range;

    // Calculate the offset
    let offset = (-leap_min * scale) + BASE_NOTE as f32;

    // Apply the mapping formula
    let midi_value = (value * scale + offset).round() as u8;

    return midi_value.clamp(0, 127);
}

fn send_midi_chord_on(
    output_port: &mut MidiOutputConnection,
    note: u8,
    velocity: u8,
    program: u8,
    attack: u8,
    decay: u8,
    sustain: u8,
    release: u8,
    scale: &Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    let program_change_status = 0xC0 | (MIDI_CHANNEL - 1);
    output_port.send(&[program_change_status, program])?;

    send_midi_cc(output_port, 1, attack)?;
    send_midi_cc(output_port, 2, decay)?;
    send_midi_cc(output_port, 3, sustain)?;
    send_midi_cc(output_port, 4, release)?;

    for &offset in &[6, 0, 2, 4] {
        // Send MIDI note-on messages for the notes of the chord
        
        let scale_index = scale.binary_search(&note).unwrap();
        let chord_note = scale[scale_index + offset];
        println!("note: {:?}, chord_note {}, {:?}", note, chord_note, midi_to_note_name(chord_note).unwrap());
        let note_on_status = 0x90 | (MIDI_CHANNEL - 1);
        output_port.send(&[note_on_status, chord_note, velocity])?;
    }

    Ok(())
}

fn send_midi_note_off(
    output_port: &mut MidiOutputConnection,
    note: u8,
    velocity: u8,
) -> Result<(), Box<dyn Error>> {
    let note_off_status = 0x80 | (MIDI_CHANNEL - 1);
    output_port.send(&[note_off_status, note, velocity])?;
    Ok(())
}

fn send_midi_cc(
    output_port: &mut MidiOutputConnection,
    cc_number: u8,
    value: u8,
) -> Result<(), Box<dyn Error>> {
    let cc_status = 0xB0 | (MIDI_CHANNEL - 1);
    output_port.send(&[cc_status, cc_number, value])?;
    Ok(())
}

fn change_instrument(
    output_port: &mut MidiOutputConnection,
    channel: u8,
    program: u8,
) -> Result<(), Box<dyn Error>> {
    // Send "note-off" messages for all possible notes on the channel
    for note in 0..127 {
        let note_off_status = 0x80 | (channel - 1);
        output_port.send(&[note_off_status, note, 0])?;
    }
    send_midi_cc(output_port, 123, 0).ok();

    let status_byte = 0xC0 | (channel - 1);
    output_port.send(&[status_byte, program])?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    // loggings

    // Initialize SCALE using the generate_scale function
    let scale: Vec<u8> = generate_minor_scale(3);
    let mut scale_notenames: Vec<String> = Vec::new();
    // for each note in scale, compare it from midi_to_note_name()
    for midi_value in &scale {
        if let Some(note_name) = midi_to_note_name(*midi_value) {
            scale_notenames.push(note_name);
        }
    }

    let mut program = 0;

    let mut moving_average = MovingAverage::new();
    let mut last_note_time = Instant::now();
    let mut last_hand_position = (0, 0);
    let mut last_program_change_time = Instant::now();

    let mouse = Mouse::new();
    let midi_out = MidiOutput::new("epicness")?;
    let output_ports = midi_out.ports();
    let mut output_port: MidiOutputConnection = midi_out.connect(&output_ports[0], "epicness")?;
    change_instrument(&mut output_port, MIDI_CHANNEL, program).unwrap();

    let mut connection =
        Connection::create(ConnectionConfig::default()).expect("Failed to create connection");
    connection.open().expect("Failed to open the connection");

    let mut active_notes: HashMap<u8, Instant> = HashMap::new();

    loop {
        let message = connection.poll(10_000)?;
        if let Event::Tracking(data) = message.event() {
            for hand in data
                .hands()
                .iter()
                .filter(|hand| hand.hand_type() == HandType::Right)
            {
                let now = Instant::now();

                let (leap_x, leap_y, leap_z) = hand_tracking(hand, &mut moving_average);
                let (screen_x, screen_y) =
                    map_leap_coordinates_to_screen(leap_x as f32, leap_y as f32);

                mouse.move_to(screen_x, screen_y).unwrap();

                let movement = (leap_x - last_hand_position.0, leap_y - last_hand_position.1);
                //let speed = (((movement.0.pow(2)) - movement.1.pow(2)) as f64).sqrt();

                let rate = (1.000 - hand.palm().orientation().z().abs()).clamp(0.1, 0.80) as f64;
                let midi_delay = MIDI_DELAY_MS as f64 * rate;

                if now.duration_since(last_note_time).as_millis() >= midi_delay as u128
                    && movement != (0, 0)
                {
                    let note = map_to_midi(leap_x as f32, MIN_X, MAX_X as f32, scale.len() as f32);
                    let velocity =
                        map_to_midi(leap_y as f32, MIN_Y, MAX_Y as f32, 127.0);
                    let depth: u8 =
                        map_to_midi(leap_z as f32, MIN_Z, MAX_Z as f32, scale.len() as f32);
                    let nearest_note = find_nearest_note_in_scale(note, &scale);
                    
                    execute!(std::io::stdout(), Clear(ClearType::All)).unwrap();

                    println!(
                        "
Scale = {:?}

Note names = {:?}

Scale minimum {}
Scale maximum {}
Scale size {}

Sound = {}
Scale'd Midi note = {}, {:?}
Velocity = {}
Depth = {}
Pinch distance = {}
Wait unit: {}

Leap X {}, Screen X {}
Leap Y {}, Screen Y {}
Leap Z {}, Depth    {}
                        ",
                        //scale.iter().map(|&x| x.to_string()).collect::<Vec<String>>().join(", "),
                        scale,
                        scale_notenames,
                        scale.first().unwrap(),
                        scale.last().unwrap(),
                        scale.len(),
                        program,
                        nearest_note, midi_to_note_name(nearest_note).unwrap(),
                        velocity,
                        depth,
                        hand.pinch_distance(),
                        rate,
                        leap_x,
                        screen_x,
                        leap_y,
                        screen_y,
                        leap_z,
                        depth,
                    );

                    // Calculate note duration based on velocity
                    let max_duration = 5_000; // Maximum duration in milliseconds
                    let min_duration = 100; // Minimum duration in milliseconds
                    let velocity_range = 127; // Maximum MIDI velocity

                    // Calculate note duration as a function of velocity
                    let duration = max_duration
                        - ((velocity as u64 * (max_duration - min_duration))
                            / velocity_range as u64);

                    let velocity = velocity.min(127);

                    send_midi_chord_on(
                        &mut output_port,
                        nearest_note,
                        velocity,
                        program,
                        70,
                        100,
                        80 as u8,
                        duration as u8,
                        &scale
                    )
                    .ok();
                    // Send MIDI CC message for depth
                    send_midi_cc(&mut output_port, 74, velocity as u8)?; // cutoff
                    send_midi_cc(&mut output_port, 91, velocity as u8)?; // reverb
                    send_midi_cc(&mut output_port, 92, duration as u8)?; // reverb
                    send_midi_cc(&mut output_port, 1, depth as u8)?; // modulation

                    // Store the note and its off-time in the HashMap
                    active_notes.insert(nearest_note, now + Duration::from_millis(duration));
                    last_note_time = now;
                }

                // Check and send note-off messages for notes that have expired
                let notes_to_remove: Vec<u8> = active_notes
                    .iter()
                    .filter_map(
                        |(&note, &off_time)| {
                            if now >= off_time {
                                Some(note)
                            } else {
                                None
                            }
                        },
                    )
                    .collect();

                //GESTURES
                if hand.pinch_distance() < 10.0
                    && now.duration_since(last_program_change_time).as_secs() >= 1
                {
                    program = (program + 1) % 128; // Increment program cyclically
                    change_instrument(&mut output_port, MIDI_CHANNEL, program).unwrap();
                    last_program_change_time = now;
                }

                for note in notes_to_remove {
                    send_midi_note_off(&mut output_port, note, 0).ok();
                    active_notes.remove(&note);
                }
                last_hand_position = (leap_x, leap_y);
                //loading_done = true;
            }
            if program == 200{break}
            
        }
    }
    
    Ok(())
}

fn midi_to_note_name(midi_value: u8) -> Option<String> {
    let note_names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];

    if (0..128).contains(&midi_value) {
        let note_index = (midi_value % 12) as usize;
        let octave = (midi_value / 12) - 1;
        let note_name = note_names[note_index];
        let full_note_name = format!("{}{}", note_name, octave);
        Some(full_note_name)
    } else {
        None
    }
}


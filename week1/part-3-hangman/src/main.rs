// Simple Hangman Program
// User gets five incorrect guesses
// Word chosen randomly from words.txt
// Inspiration from: https://doc.rust-lang.org/book/ch02-00-guessing-game-tutorial.html
// This assignment will introduce you to some fundamental syntax in Rust:
// - variable declaration
// - string manipulation
// - conditional statements
// - loops
// - vectors
// - files
// - user input
// We've tried to limit/hide Rust's quirks since we'll discuss those details
// more in depth in the coming lectures.
extern crate rand;
use rand::Rng;
use std::fs;
use std::io;
use std::io::Write;

const NUM_INCORRECT_GUESSES: u32 = 5;
const WORDS_PATH: &str = "words.txt";

fn pick_a_random_word() -> String {
    let file_string = fs::read_to_string(WORDS_PATH).expect("Unable to read file.");
    let words: Vec<&str> = file_string.split('\n').collect();
    String::from(words[rand::thread_rng().gen_range(0, words.len())].trim())
}

fn main() {
    let secret_word = pick_a_random_word();
    // Note: given what you know about Rust so far, it's easier to pull characters out of a
    // vector than it is to pull them out of a string. You can get the ith character of
    // secret_word by doing secret_word_chars[i].
    let secret_word_chars: Vec<char> = secret_word.chars().collect();
    // Uncomment for debugging:
    // println!("random word: {}", secret_word);

    // Your code here! :)
    println!("Welcome to CS110L Hangman!");
    let mut incorrect_guesses = 0;
    let mut known_chars = vec!['-'; secret_word_chars.len()];
    let mut guessed_letters = Vec::new();
    while incorrect_guesses < NUM_INCORRECT_GUESSES {
        print!("The word so far is ");
        for i in known_chars.iter() {
            print!("{}", i);
        }
        println!();
        print!("You have guessed the following letters: ");
        for i in guessed_letters.iter() {
            print!("{}", i);
        }
        println!();
        println!("You have {} guesses left", NUM_INCORRECT_GUESSES - incorrect_guesses);
        print!("Please guess a letter: ");
        io::stdout()
            .flush()
            .expect("Error flushing stdout.");
        let mut guess = String::new();
        io::stdin()
            .read_line(&mut guess)
            .expect("Error reading line.");
        let guess_char = guess.as_bytes()[0] as char;
        guessed_letters.push(guess_char);
        let mut in_string = false;
        let mut idx = 0;
        while idx < known_chars.len() {
            if guess_char == secret_word_chars[idx] {
                known_chars[idx] = guess_char;
                in_string = true;
            }
            idx += 1;
        }
        if !in_string {
            incorrect_guesses += 1;
            println!("Sorry, that letter is not in the word");
        }
        println!();
        let mut remain_unknown = false;
        for i in known_chars.iter() {
            if *i == '-' {
                remain_unknown = true;
                break;
            }
        }
        if !remain_unknown {
            break;
        }
    }
    if incorrect_guesses < NUM_INCORRECT_GUESSES {
        println!("Congratulations you guessed the secret word: {}!", secret_word);
    } else {
        println!("Sorry, you ran out of guesses!");
    }
}

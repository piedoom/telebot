use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::Duration;
use std::{env, thread};

use derive_more::From;
use once_cell::sync::OnceCell;
use rand::prelude::IteratorRandom;
use teloxide::macros::Transition;
use teloxide::prelude::*;

// We use a BTree to keep insertions/deletions cheap
/// List of words that can be used by the game
static GAME_WORDS: OnceCell<RwLock<BTreeSet<String>>> = OnceCell::new();
/// List of words that won't be used by the game, but can be guessed by a player
static DICT_WORDS: OnceCell<RwLock<BTreeSet<String>>> = OnceCell::new();
/// Flag to indicate to our worker thread that the dictionary has been updated
static DIRTY_DICTIONARY: OnceCell<AtomicBool> = OnceCell::new();
/// Flag to indicate to our worker thread that the process is exiting
static APP_EXITING: OnceCell<AtomicBool> = OnceCell::new();

#[tokio::main]
async fn main() {
    // Load the dictionaries first
    load_game_words();
    load_dict_words();
    DIRTY_DICTIONARY
        .set(AtomicBool::new(false))
        .expect("could not initialize DIRTY_DICTIONARY");
    APP_EXITING
        .set(AtomicBool::new(false))
        .expect("could not initialize DIRTY_DICTIONARY");

    // Start a background thread that waits for the dictionary to be edited
    let background_thread = thread::spawn(dictionary_worker);

    run().await;
    APP_EXITING.get().unwrap().store(true, Ordering::Relaxed);
    background_thread
        .join()
        .expect("failed to join background thread");
}

fn assets_dir() -> PathBuf {
    Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets")
}

fn dictionary_worker() {
    let app_exiting = APP_EXITING.get().unwrap();
    let dirty_dictionary = DIRTY_DICTIONARY.get().unwrap();

    while !app_exiting.load(Ordering::Relaxed) {
        if dirty_dictionary.swap(false, Ordering::Relaxed) {
            // The dictionary has been updated. We need to serialize both
            println!("Updating word lists");
            let dictionaries: [_; 2] = [
                (&GAME_WORDS, assets_dir().join("words_custom.txt")),
                (&DICT_WORDS, assets_dir().join("dictionary_custom.txt")),
            ];
            for (dict, file_path) in dictionaries {
                let dict = dict.get().expect("dictionary not initialized");
                let dict = dict.read().expect("could not lock dictionary");

                let output_file =
                    File::create(file_path).expect("could not create custom dictoinary file");
                let mut output_file = LineWriter::new(output_file);

                for word in &*dict {
                    output_file
                        .write_all(word.as_bytes())
                        .expect("failed to write custom word");
                    output_file
                        .write_all("\n".as_bytes())
                        .expect("failed to write newline");
                }
            }
        }

        // Wait 5m
        thread::sleep(Duration::from_secs(2 * 60));
    }
}

fn load_game_words() {
    let mut btree = BTreeSet::default();
    let assets_dir = assets_dir();

    let file = if assets_dir.join("words_custom.txt").exists() {
        File::open(assets_dir.join("words_custom.txt")).expect("no such file")
    } else {
        File::open(assets_dir.join("words.txt")).expect("no such file")
    };

    let buf = BufReader::new(file);
    for line in buf.lines() {
        btree.insert(line.expect("could not parse line"));
    }

    GAME_WORDS
        .set(RwLock::new(btree))
        .expect("GAME_WORDS already initialized")
}

fn load_dict_words() {
    let mut btree = BTreeSet::default();
    let assets_dir = assets_dir();

    let file = if assets_dir.join("dictionary_custom.txt").exists() {
        File::open(assets_dir.join("dictionary_custom.txt")).expect("no such file")
    } else {
        File::open(assets_dir.join("dictionary.txt")).expect("no such file")
    };

    let buf = BufReader::new(file);
    for line in buf.lines() {
        btree.insert(line.expect("could not parse line"));
    }

    DICT_WORDS
        .set(RwLock::new(btree))
        .expect("DICT_WORDS already initialized")
}

fn get_random_word() -> String {
    let game_words = GAME_WORDS.get().expect("GAME_WORDS is not initialized");
    let game_words = game_words.read().expect("failed to lock GAME_WORDS");
    game_words
        .iter()
        .choose(&mut rand::thread_rng())
        .unwrap()
        .clone()
}

fn is_dictionary_word(word: &str) -> bool {
    let dict_words = DICT_WORDS.get().expect("DICT_WORDS is not initialized");
    let dict_words = dict_words.read().expect("failed to lock DICT_WORDS");

    dict_words.contains(word)
}

async fn run() {
    teloxide::enable_logging!();
    log::info!("Starting bot...");
    dotenv::dotenv().ok();

    let bot = Bot::from_env().auto_send();

    teloxide::dialogues_repl(bot, |message, dialogue| async move {
        handle_message(message, dialogue)
            .await
            .expect("Something wrong with the bot!")
    })
    .await;
}

async fn handle_message(
    cx: UpdateWithCx<AutoSend<Bot>, Message>,
    dialogue: Dialogue,
) -> TransitionOut<Dialogue> {
    match cx.update.text().map(ToOwned::to_owned) {
        None => next(dialogue),
        Some(ans) => dialogue.react(cx, ans).await,
    }
}

#[derive(From, Transition, Clone)]
pub enum Dialogue {
    Start(StartState),
    Guess(GuessState),
}

impl Default for Dialogue {
    fn default() -> Self {
        Self::Start(StartState)
    }
}

pub enum DictionaryAction<'a> {
    Add(&'a [&'a str]),
    Remove(&'a [&'a str]),
}

async fn edit_dictionary(action: DictionaryAction<'_>, cx: TransitionIn<AutoSend<Bot>>) {
    //-> AutoRequest<JsonRequest<SendMessage>> {
    let dirty_dictionary = DIRTY_DICTIONARY.get().unwrap();
    match action {
        DictionaryAction::Add(words) => {
            let mut added_words = BTreeSet::new();

            let dictionaries: [_; 2] = [&GAME_WORDS, &DICT_WORDS];
            for dict in dictionaries {
                let dict = dict.get().expect("dictionary not initialized");
                let mut dict = dict.write().expect("could not lock dictionary");

                for word in words {
                    if word.len() != 5 {
                        continue;
                    }

                    if dict.insert(word.to_string()) {
                        added_words.insert(*word);
                    }
                }
            }
            dirty_dictionary.store(true, Ordering::Relaxed);
            cx.answer(format!("Added {:?}", added_words)).await.ok();
        }
        DictionaryAction::Remove(words) => {
            let mut removed_words = BTreeSet::new();

            let dictionaries: [_; 2] = [&GAME_WORDS, &DICT_WORDS];
            for dict in dictionaries {
                let dict = dict.get().expect("dictionary not initialized");
                let mut dict = dict.write().expect("could not lock dictionary");

                for word in words {
                    if dict.remove(*word) {
                        removed_words.insert(*word);
                    }
                }
            }
            dirty_dictionary.store(true, Ordering::Relaxed);
            cx.answer(format!("Removed {:?}", removed_words)).await.ok();
        }
    }
}

#[derive(Clone)]
pub struct StartState;

#[teloxide(subtransition)]
async fn start_state(
    state: StartState,
    cx: TransitionIn<AutoSend<Bot>>,
    ans: String,
) -> TransitionOut<Dialogue> {
    let input: Vec<String> = ans.split_whitespace().map(String::from).collect();
    match ans.as_str() {
        "/wordle" => {
            cx.answer("Wordle game started - /guess any 5 letter word")
                .await?;
            next(GuessState {
                answer: get_random_word(),
                guesses: Default::default(),
                last_input: input,
            })
        }
        "/420" => {
            "heh";
            next(state)
        }
        _ => next(state),
    }
}

#[derive(Clone)]
pub struct GuessState {
    pub answer: String,
    // Emoji representation as well as word guessed
    pub guesses: Vec<(String, String)>,
    pub last_input: Vec<String>,
}

#[teloxide(subtransition)]
async fn guess_state(
    state: GuessState,
    cx: TransitionIn<AutoSend<Bot>>,
    ans: String,
) -> TransitionOut<Dialogue> {
    let input: Vec<String> = ans.split_whitespace().map(String::from).collect();
    let input_str: Vec<&str> = input.iter().map(String::as_str).collect();

    let mut new_state = state.clone();
    new_state.last_input = input.clone();

    match new_state.last_input[0].as_str() {
        "/addword" | "/addword@doomybot" => {
            let wants_to_add_previous_guess = input.len() == 1 && state.last_input.len() == 2;

            if wants_to_add_previous_guess {
                edit_dictionary(DictionaryAction::Add(&[&state.last_input[1]]), cx).await;
            } else {
                edit_dictionary(DictionaryAction::Add(&input_str[1..]), cx).await;
            }

            next(new_state)
        }
        "/exit" | "/end" | "/stop" => {
            let word = state.answer;
            cx.answer(format!("Ending game. Word was {word}")).await?;
            next(StartState)
        }
        "/removeword" => {
            if input.len() < 2 {
                cx.answer("Usage: /removeword <WORD> [..WORD2]").await?;
            } else {
                edit_dictionary(DictionaryAction::Remove(&input_str[1..]), cx).await;
            }

            next(new_state)
        }
        "/guess" if input.len() == 2 => {
            let attempt = input_str[1];
            let answer = &state.answer;

            let mut placement = [Placement::Missing; 5];

            // return early if length of attempt is wrong amount of characters
            if attempt.chars().count() != 5 {
                cx.answer("Guess was not 5 characters").await.ok();
                return next(new_state);
            }

            if !is_dictionary_word(attempt) {
                cx.answer(format!("{attempt} is not in the dictionary. /addword?"))
                    .await
                    .ok();
                return next(new_state);
            }

            let mut corrected_answer: Vec<char> = answer.clone().chars().collect();

            // check for correct placement
            attempt.chars().zip(answer.chars()).enumerate().for_each(
                |(i, (attempt_char, answer_char))| {
                    if attempt_char == answer_char {
                        placement[i] = Placement::Correct;
                        // remove the char from our corrected_answer so we can check for misplaced chars without dupes
                        corrected_answer[i] = ' ';
                    }
                },
            );

            // check for misplaced characters
            attempt.chars().enumerate().for_each(|(i, attempt_char)| {
                if placement[i] != Placement::Correct && corrected_answer.contains(&attempt_char) {
                    placement[i] = Placement::Incorrect;
                }
            });

            // get the answer
            let result = to_emoji(&placement);

            // add to our guess history
            let mut guesses = state.guesses.clone();
            guesses.push((result, attempt.to_string()));
            let emoji_string = guesses
                .iter()
                .map(|(a, _)| a.clone())
                .collect::<Vec<String>>()
                .join("\n");

            let tries = guesses.len();
            // if we won...
            match placement == [Placement::Correct; 5] {
                true => {
                    cx.answer(format!("You won. {tries}/6\n{emoji_string}"))
                        .await
                        .ok();
                    next(StartState)
                }
                false => {
                    // check to see if we're out of guesses
                    let next_guess = tries + 1;
                    if next_guess < 7 {
                        cx.answer(format!("{tries}/6\n{emoji_string}")).await.ok();
                        next(GuessState {
                            answer: answer.to_string(),
                            guesses,
                            last_input: input,
                        })
                    } else {
                        // lost
                        let answer = state.answer;
                        cx.answer(format!(
                            "You lost. 6/6. Cringe.\nAnswer was {answer}\n{emoji_string}"
                        ))
                        .await
                        .ok();
                        next(StartState)
                    }
                }
            }
        }
        "/guess" => {
            cx.answer("Invalid guess");
            next(state)
        }
        _ => {
            // Not meant for us?
            next(state)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Correct,
    Incorrect,
    Missing,
}

fn to_emoji(placement: &[Placement]) -> String {
    placement
        .iter()
        .map(|p| match p {
            Placement::Correct => '🟩',
            Placement::Incorrect => '🟨',
            Placement::Missing => '⬛',
        })
        .collect()
}

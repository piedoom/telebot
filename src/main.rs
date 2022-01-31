use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use derive_more::From;
use rand::prelude::IteratorRandom;
use teloxide::macros::Transition;
use teloxide::prelude::*;

#[tokio::main]
async fn main() {
    run().await;
}

fn get_random_word() -> String {
    let assets_dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    let file = File::open(assets_dir.join("words.txt")).expect("no such file");
    let buf = BufReader::new(file);
    buf.lines()
        .map(|l| l.expect("Could not parse line"))
        .choose(&mut rand::thread_rng())
        .unwrap()
}

fn is_dictionary_word(word: &str) -> bool {
    let assets_dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    let file = File::open(assets_dir.join("dictionary.txt")).expect("no such file");
    let buf = BufReader::new(file);
    buf.lines()
        .map(|l| l.expect("Could not parse line"))
        .any(|x| x == word)
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

#[derive(Clone)]
pub struct StartState;

#[teloxide(subtransition)]
async fn start_state(
    state: StartState,
    cx: TransitionIn<AutoSend<Bot>>,
    ans: String,
) -> TransitionOut<Dialogue> {
    match ans.as_str() {
        "/wordle" => {
            cx.answer("Wordle game started - /guess any 5 letter word")
                .await?;
            next(GuessState {
                answer: get_random_word(),
                guesses: Default::default(),
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
    pub guesses: Vec<String>,
}

#[teloxide(subtransition)]
async fn guess_state(
    state: GuessState,
    cx: TransitionIn<AutoSend<Bot>>,
    ans: String,
) -> TransitionOut<Dialogue> {
    let ans: Vec<&str> = ans.split_whitespace().collect();
    if ans.len() == 2 && ans[0] == "/guess" {
        let attempt = ans[1];
        let answer = &state.answer;

        let mut placement = [Placement::Missing; 5];

        // return early if length of attempt is wrong amount of characters
        if attempt.chars().count() != 5 {
            cx.answer("Guess was not 5 characters").await.ok();
            return next(state);
        }

        if !is_dictionary_word(attempt) {
            cx.answer(format!("{attempt} is not in the dictionary"))
                .await
                .ok();
            return next(state);
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
        guesses.push(result);
        let guesses_string = guesses.join("\n");

        let tries = guesses.len();
        // if we won...
        match placement == [Placement::Correct; 5] {
            true => {
                cx.answer(format!("You won. {tries}/6\n{guesses_string}"))
                    .await
                    .ok();
                next(StartState)
            }
            false => {
                // check to see if we're out of guesses
                let next_guess = tries + 1;
                if next_guess < 7 {
                    cx.answer(format!("{tries}/6\n{guesses_string}")).await.ok();
                    next(GuessState {
                        answer: answer.to_string(),
                        guesses,
                    })
                } else {
                    // lost
                    let answer = state.answer;
                    cx.answer(format!(
                        "You lost. 6/6. Cringe.\nAnswer was {answer}\n{guesses_string}"
                    ))
                    .await
                    .ok();
                    next(StartState)
                }
            }
        }
    } else {
        cx.answer("Invalid guess");
        next(state)
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

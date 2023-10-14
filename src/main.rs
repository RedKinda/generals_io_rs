use std::{
    cmp::max,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use client::LobbyType;
use mcts::MctsTree;
use oxymcts::Evaluator;
use state::{GameState, GeneralsGameState, Tile, TileType};
use utils::int_to_location;

use crate::{
    constants::{load_env_vars, THINKING_TIME},
    mcts::{evaluate_state, GeneralsUctEvaluator},
    state::SerializedMoveCommand,
    utils::location_to_int,
};

pub mod client;
pub mod constants;
pub mod enemy;
pub mod mcts;
pub mod state;
pub mod utils;

#[macro_use]
extern crate serde;

#[macro_use]
extern crate serde_tuple;

#[macro_use]
extern crate tracing;

#[tokio::main]
async fn main() {
    // setup tracing subscriber
    tracing_subscriber::fmt::init();

    // read game id from stdin
    // let mut gameid = String::new();
    // std::io::stdin().read_line(&mut gameid).unwrap();

    let (userid, username, gameid) = load_env_vars();

    let lobby_type = if let Some(gameid) = gameid {
        LobbyType::Private(gameid)
    } else {
        LobbyType::OneVOne
    };

    info!("userid: {}", userid);

    loop {
        let mut client = client::GeneralsClient::connect(&userid, &username, &lobby_type).await;

        let game_start = client.wait_game_start().await;
        let mut update = client.get_game_update().await.unwrap();

        let mut update_received_time = std::time::Instant::now();

        info!("game start: {:?}", game_start);

        // since generals gives us variable map size, we pad it to fit within 25x25
        let width = update.map_diff[2] as u64;
        let height = update.map_diff[3] as u64;

        let left_padding = (25 - width) / 2;
        let top_padding = (25 - height) / 2;

        let mut game: GeneralsGameState = GameState::new(
            game_start.player_index,
            int_to_location(
                update.generals[game_start.player_index as usize] as u64,
                width,
                height,
                left_padding,
                top_padding,
            ),
        );

        // set all padded tiles to visible mountains
        for x in 0..25 {
            for y in 0..25 {
                if x < left_padding
                    || x >= left_padding + width
                    || y < top_padding
                    || y >= top_padding + height
                {
                    game = game.update_tile(
                        (x as usize, y as usize),
                        Tile::new(TileType::Padding, 0, None),
                    );
                    game.fog_mask[x as usize][y as usize] = 1;
                }
            }
        }

        // first update sucks because index 2 and 3 are width and height
        // so we skip it for the first update, and lower index 1 by 2
        update.map_diff[1] -= 2;
        update.map_diff.remove(2);
        update.map_diff.remove(2);

        let mut first_update = true;
        let mut estimated_next_state: Option<GeneralsGameState> = None;

        let mut mcts = Arc::new(MctsTree::new(&game));
        let mcts_interruptor = Arc::new(AtomicBool::new(false));
        let mut estimated_correct = 0;
        let mut total_updates = 0;

        loop {
            // apply update
            let mut diff_offset = 0;
            let mut cache_offset = 0;
            if !first_update {
                update.map_diff[0] -= 2;
            }

            while diff_offset < update.map_diff.len() - 1 {
                cache_offset += update.map_diff[diff_offset] as usize;
                let length = (update.map_diff[diff_offset + 1]) as usize;

                for i in 0..length {
                    let mut army_diff = cache_offset + i < (width * height) as usize;

                    let mut location = int_to_location(
                        (cache_offset + i) as u64,
                        width,
                        height,
                        left_padding,
                        top_padding,
                    );

                    // if location.1 as u64 > height + top_padding {
                    //     army_diff = false;
                    //     // panic!("location out of bounds: {:?}", location);
                    // }

                    if army_diff {
                        game = game.change_tile_population(
                            location,
                            update.map_diff[diff_offset + i + 2]
                                - game.get_tile(location).population as i16,
                        );
                    } else {
                        let new_type = update.map_diff[diff_offset + i + 2];
                        let previous_tile = game.get_tile(location).clone();
                        let new_tile = match new_type {
                            -1 => {
                                // confirmed empty
                                Tile::new(
                                    TileType::VisibleEmpty,
                                    previous_tile.population,
                                    previous_tile.owner,
                                )
                            }
                            -2 => {
                                // confirmed mountain
                                Tile::new(
                                    TileType::VisibleMountain,
                                    previous_tile.population,
                                    previous_tile.owner,
                                )
                            }
                            -3 => {
                                // fog
                                Tile::new(
                                    previous_tile.tile_type.hide(),
                                    previous_tile.population,
                                    previous_tile.owner,
                                )
                            }
                            -4 => {
                                // HiddenObstacle
                                Tile::new(
                                    TileType::HiddenObstacle,
                                    previous_tile.population,
                                    previous_tile.owner,
                                )
                            }
                            owner => {
                                // visible tile
                                let owner = owner as u8;
                                let owned = owner == game_start.player_index;

                                if previous_tile.owner != Some(owner) {
                                    game = game.change_tile_ownership(
                                        location,
                                        owner,
                                        previous_tile.population,
                                    );
                                }

                                Tile::new(
                                    if owned {
                                        previous_tile.tile_type.own()
                                    } else {
                                        previous_tile.tile_type.lose()
                                    },
                                    previous_tile.population,
                                    Some(owner),
                                )
                            }
                        };

                        game = game.update_tile(location, new_tile);
                    }
                }

                diff_offset += length + 2;
                cache_offset += length;
            }

            // now we get city diff, which is the same as map diff but we change tile types to cities
            diff_offset = 0;
            cache_offset = 0;

            while diff_offset < update.cities_diff.len() - 1 {
                cache_offset += update.cities_diff[diff_offset] as usize;
                let length = (update.cities_diff[diff_offset + 1]) as usize;

                for i in 0..length {
                    let location = int_to_location(
                        update.cities_diff[diff_offset + i + 2] as u64,
                        width,
                        height,
                        left_padding,
                        top_padding,
                    );

                    let mut previous_tile = game.get_tile(location).clone();

                    let tile_type = if previous_tile.owner.is_some() {
                        TileType::EnemyCity
                    } else {
                        TileType::VisibleNeutralCity
                    };
                    previous_tile.tile_type = tile_type;

                    game = game.update_tile(location, previous_tile);
                }

                diff_offset += length + 2;
                cache_offset += length;
            }

            game.turn = update.turn;

            for score in update.scores {
                if game.turn % 50 != 0 {
                    let army_diff =
                        score.army_count as i32 - game.armies[score.player_index as usize] as i32;

                    if army_diff == game.city_count[score.player_index as usize] as i32 + 1 {
                        game.city_count[score.player_index as usize] = army_diff as u16;
                    }
                }

                game.armies[score.player_index as usize] = score.army_count;
                game.lands[score.player_index as usize] = score.tile_count;
            }

            if let Some(estimate) = estimated_next_state {
                let estimate_hash = estimate.get_hash();
                let actual_hash = game.get_hash();

                if estimate_hash != actual_hash {
                    info!("estimated hash: {}", estimate_hash);
                    info!("actual hash: {}", actual_hash);

                    client.clear_commands().await;
                    // warn!(
                    //     "hashes do not match, we are out of sync\nEstimate: {}\n\nActual: {}",
                    //     estimate, game
                    // );

                    mcts = Arc::new(MctsTree::new(&game));
                } else {
                    estimated_correct += 1;
                }
            } else {
                mcts = Arc::new(MctsTree::new(&game));
            }

            total_updates += 1;

            let moves = game.get_possible_commands();
            //&& !first_update
            if !moves.is_empty() {
                // // select random one
                // let move_index = rand::random::<usize>() % moves.len();
                // let move_command = moves[move_index].clone();
                let move_command = mcts
                    .get_best_move(Duration::from_millis(THINKING_TIME))
                    .await;

                let ser = SerializedMoveCommand {
                    from: location_to_int(
                        move_command.from,
                        width,
                        height,
                        left_padding,
                        top_padding,
                    ),
                    to: location_to_int(move_command.to, width, height, left_padding, top_padding),
                    half: move_command.half,
                };

                if move_command.from == move_command.to {
                    info!("We did a NOOP")
                } else {
                    info!("sending move: {:?}", move_command);
                }

                estimated_next_state = Some(game.tick(&move_command).unwrap());

                let score = evaluate_state(estimated_next_state.as_ref().unwrap().clone());

                info!(
                    "{}\nscore:{}",
                    estimated_next_state.as_ref().unwrap(),
                    score
                );

                client.send_cmd(ser).await;

                mcts = Arc::new(MctsTree::new(estimated_next_state.as_ref().unwrap()));
                mcts_interruptor.store(false, std::sync::atomic::Ordering::SeqCst);
                let local_mcts = mcts.clone();
                let local_interruptor = mcts_interruptor.clone();
                tokio::spawn(
                    async move { local_mcts.train_until_interrupt(local_interruptor).await },
                );
            } else {
                estimated_next_state = None;
                info!("{}", game);
            }

            first_update = false;
            // measure time waiting for update
            let start = std::time::Instant::now();
            let recv = client.get_game_update().await;
            if recv.is_none() {
                break;
            }
            update = recv.unwrap();

            mcts_interruptor.store(true, std::sync::atomic::Ordering::SeqCst);

            let end = std::time::Instant::now();
            let waited = end - start;
            let time_between_updates = end - update_received_time;
            update_received_time = end;

            info!(
                "waited for update: {:?}, time between updates: {:?}",
                waited, time_between_updates
            );
        }

        mcts_interruptor.store(true, std::sync::atomic::Ordering::SeqCst);
        info!("estimated correct: {}/{}", estimated_correct, total_updates);
    }
}

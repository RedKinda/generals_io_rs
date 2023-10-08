// here is a simple logic for estimating what goes on in the enemy mind

use rand::seq::SliceRandom;

use crate::{
    state::{GeneralsGameState, Location, MoveCommand, PlayerId, Tile},
    utils::{get_neighbors, manhattan_distance},
};

#[derive(Debug, Clone, Hash)]
pub enum EnemyMove {
    Noop,
    ExpandLand,
    Invasion { from: Location, to: Location },
}

impl EnemyMove {
    pub fn apply_on_state(
        &self,
        state: &GeneralsGameState,
        player_id: PlayerId,
    ) -> GeneralsGameState {
        match self {
            EnemyMove::Noop => state.clone(),
            EnemyMove::ExpandLand => {
                let mut state = state.clone();
                state.lands[player_id as usize] += 1;
                state
            }
            EnemyMove::Invasion { from, to } => state
                .process_command(
                    &MoveCommand {
                        from: *from,
                        to: *to,
                        half: false,
                    },
                    player_id,
                    true,
                )
                .unwrap(),
        }
    }
}

pub fn possible_enemy_moves(state: &GeneralsGameState, player_id: PlayerId) -> Vec<EnemyMove> {
    // enemy can always do nothing
    let mut moves = Vec::with_capacity(8);
    moves.push(EnemyMove::Noop);

    // if enemy has army > land, they can expand land
    if state.armies[player_id as usize] > state.lands[player_id as usize] {
        moves.push(EnemyMove::ExpandLand);
    }

    // if enemy has a field with >=3 army that we know of, they can invade towards our general
    // there can be max 3 possible invader armies, those with max value
    let tiles: &[[Tile; 25]; 25] = state.tiles();

    let mut invader_armies = vec![];
    let mut possible_invade_spots = vec![];

    let fictional_army_size = if state.armies[player_id as usize]
        > state.lands[player_id as usize] + 1
    {
        let s = ((state.armies[player_id as usize] - state.lands[player_id as usize]) as f32 * 0.8)
            .round() as u16;
        if s > 10 {
            Some(s)
        } else {
            None
        }
    } else {
        None
    };

    if state.turn <= 50 {
        // enemies cant invade this early in the game
        return moves;
    }

    for x in 0..25 {
        for y in 0..25 {
            let tile = tiles[x][y];
            if tile.tile_type.is_enemy() {
                if tile.population >= 3 {
                    invader_armies.push((tile.population, (x, y)));
                }
            } else if !tile.tile_type.is_owned()
                && tile.tile_type.occupiable()
                && state.fog_mask[x][y] > 0
            {
                possible_invade_spots.push((x, y));
            }
        }
    }

    invader_armies.sort_by_key(|(pop, _)| *pop);
    invader_armies.reverse();

    // if biggest army is smaller than fictional/2, we prepend it to the list
    if fictional_army_size.is_some()
        && (invader_armies.len() == 0
            || tiles[invader_armies[0].1 .0][invader_armies[0].1 .1].population
                < fictional_army_size.unwrap() / 2)
    {
        //prepend random of the possible_invade_spots, choose randomly
        if let Some((x, y)) = possible_invade_spots.choose(&mut rand::thread_rng()) {
            invader_armies.insert(0, (fictional_army_size.unwrap(), (*x, *y)));
        }
    }

    // take top 1 option
    invader_armies.truncate(1);

    for (_, (x, y)) in invader_armies {
        let from = (x as i64, y as i64);
        let to = (
            state.get_own_general().0 as i64,
            state.get_own_general().1 as i64,
        );

        let mut in_movements = vec![];

        let neighbors = get_neighbors((x, y), 25, 25);

        for n in neighbors {
            if state.get_tile(n).tile_type.occupiable() {
                in_movements.push(EnemyMove::Invasion {
                    from: (x, y),
                    to: n,
                });
            }
        }

        if !in_movements.is_empty() {
            // remove the move which 'to' is furthest from our general
            let mut max_distance_ind = 0;
            let mut max_distance = 0;
            for (i, m) in in_movements.iter().enumerate() {
                let to = match m {
                    EnemyMove::Invasion { from: _, to } => *to,
                    _ => panic!("not an invasion"),
                };
                let distance = manhattan_distance(to, state.get_own_general());
                if distance > max_distance {
                    max_distance = distance;
                    max_distance_ind = i;
                }
            }

            in_movements.remove(max_distance_ind);
        }

        moves.extend(in_movements);

        // info!("Enemy can invade from {:?} to {:?}", from, to);
    }

    moves
}

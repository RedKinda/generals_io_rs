use std::{
    cmp::{max, min},
    fmt::Display,
    hash::Hasher,
};

use anyhow::Result;
use rayon::prelude::IntoParallelIterator;
use serde_json::{json, Value};
use std::hash::Hash;

use crate::{
    constants::MAX_TURNS,
    enemy::EnemyMove,
    utils::{get_neighbors, manhattan_distance},
};
pub type PlayerId = u8;
pub type Location = (usize, usize);

pub type GeneralsGameState = GameState<2, 25, 25>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum TileType {
    VisibleEmpty,
    AssumedEmpty,

    OwnedTile,
    OwnedGeneral,
    OwnedCity,

    Enemy,
    EnemyCity,
    EnemyGeneral,

    VisibleNeutralCity,
    HiddenNeutralCity,
    VisibleMountain,
    HiddenObstacle,

    Padding,
}

impl TileType {
    #[inline]
    pub fn is_visible(&self) -> bool {
        matches!(
            self,
            TileType::VisibleEmpty
                | TileType::OwnedTile
                | TileType::OwnedGeneral
                | TileType::OwnedCity
                | TileType::Enemy
                | TileType::EnemyCity
                | TileType::EnemyGeneral
                | TileType::VisibleNeutralCity
                | TileType::VisibleMountain
        )
    }
    #[inline]
    pub fn reveal(&self) -> Self {
        match self {
            TileType::AssumedEmpty => TileType::VisibleEmpty,
            TileType::HiddenNeutralCity => TileType::VisibleNeutralCity,
            TileType::HiddenObstacle => TileType::VisibleMountain,
            _ => *self,
        }
    }
    #[inline]
    pub fn hide(&self) -> Self {
        match self {
            TileType::VisibleEmpty => TileType::AssumedEmpty,
            TileType::VisibleNeutralCity => TileType::HiddenNeutralCity,
            TileType::VisibleMountain => TileType::HiddenObstacle,
            _ => *self,
        }
    }
    #[inline]
    pub fn own(&self) -> Self {
        match self {
            TileType::VisibleEmpty => TileType::OwnedTile,
            TileType::AssumedEmpty => TileType::OwnedTile,
            TileType::Enemy => TileType::OwnedTile,
            TileType::EnemyCity => TileType::OwnedCity,
            TileType::EnemyGeneral => TileType::OwnedCity,
            TileType::VisibleNeutralCity => TileType::OwnedCity,
            TileType::HiddenNeutralCity => TileType::OwnedCity,
            TileType::VisibleMountain => TileType::OwnedTile,
            TileType::HiddenObstacle => TileType::OwnedTile,
            _ => *self,
        }
    }
    #[inline]
    pub fn lose(&self) -> Self {
        match self {
            TileType::OwnedTile => TileType::Enemy,
            TileType::OwnedCity => TileType::EnemyCity,
            TileType::OwnedGeneral => TileType::EnemyCity,
            TileType::VisibleEmpty => TileType::Enemy,
            TileType::AssumedEmpty => TileType::Enemy,
            TileType::VisibleNeutralCity => TileType::EnemyCity,
            TileType::HiddenNeutralCity => TileType::EnemyCity,

            _ => *self,
        }
    }
    #[inline]
    pub fn occupiable(&self) -> bool {
        // everything that is not a mountain or obstacle
        matches!(
            self,
            TileType::VisibleEmpty
                | TileType::AssumedEmpty
                | TileType::OwnedTile
                | TileType::OwnedGeneral
                | TileType::OwnedCity
                | TileType::Enemy
                | TileType::EnemyCity
                | TileType::EnemyGeneral
                | TileType::VisibleNeutralCity
                | TileType::HiddenNeutralCity
        )
    }
    #[inline]
    pub fn is_owned(&self) -> bool {
        matches!(
            self,
            TileType::OwnedTile | TileType::OwnedGeneral | TileType::OwnedCity
        )
    }
    #[inline]
    pub fn is_enemy(&self) -> bool {
        matches!(
            self,
            TileType::Enemy | TileType::EnemyCity | TileType::EnemyGeneral
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Tile {
    pub tile_type: TileType,
    pub population: u16,
    pub owner: Option<PlayerId>,
}
impl Tile {
    pub fn new(tile_type: TileType, population: u16, owner: Option<PlayerId>) -> Self {
        Self {
            tile_type,
            population,
            owner,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct MoveCommand {
    pub from: Location,
    pub to: Location,
    pub half: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct SerializedMoveCommand {
    pub from: u64,
    pub to: u64,
    pub half: bool,
}
impl SerializedMoveCommand {
    pub fn to_json(&self, move_id: u64) -> Value {
        // self._send(["attack", a, b, move_half, self._move_id])
        json!(["attack", self.from, self.to, self.half, move_id])
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GeneralLocation {
    Known(Location),
    Dead(Location),
    Unknown,
}
impl GeneralLocation {
    pub fn unwrap_location(&self) -> Location {
        match self {
            GeneralLocation::Known(location) => *location,
            GeneralLocation::Dead(location) => *location,
            GeneralLocation::Unknown => panic!("General location is unknown"),
        }
    }
}

#[derive(Clone, Debug)]
#[must_use]
pub struct GameState<const PLAYER_COUNT: usize, const W: usize, const H: usize> {
    pub turn: u64,
    pub max_turn: u64,
    player_id: PlayerId,
    tiles: [[Tile; W]; H],
    pub fog_mask: [[u8; W]; H],

    pub lands: [u16; PLAYER_COUNT],
    pub armies: [u16; PLAYER_COUNT],
    pub city_count: [u16; PLAYER_COUNT],
    pub general_revealed_to: [bool; PLAYER_COUNT],
    generals: [GeneralLocation; PLAYER_COUNT],
}
impl<const PLAYER_COUNT: usize, const W: usize, const H: usize> GameState<PLAYER_COUNT, W, H> {
    pub fn new(player_id: PlayerId, own_general: Location) -> Self {
        let mut state = Self {
            turn: 0,
            max_turn: MAX_TURNS,
            player_id,
            tiles: [[Tile::new(TileType::AssumedEmpty, 0, None); W]; H],
            fog_mask: [[0; W]; H],
            lands: [0; PLAYER_COUNT],
            armies: [1; PLAYER_COUNT],
            city_count: [1; PLAYER_COUNT],
            general_revealed_to: [false; PLAYER_COUNT],
            generals: [GeneralLocation::Unknown; PLAYER_COUNT],
        };

        state.generals[player_id as usize] = GeneralLocation::Known(own_general);
        state.general_revealed_to[player_id as usize] = true;

        state.change_tile_ownership(own_general, player_id, 1);
        state.tiles[own_general.0][own_general.1].tile_type = TileType::OwnedGeneral;
        state
    }

    #[inline]
    pub fn get_own_general(&self) -> Location {
        self.generals[self.player_id as usize].unwrap_location()
    }

    #[inline]
    pub fn get_tile(&self, location: Location) -> &Tile {
        &self.tiles[location.0][location.1]
    }
    #[inline]
    pub fn update_tile(&self, location: Location, tile: Tile) -> Self {
        let mut new_state = self.clone();
        new_state.tiles[location.0][location.1] = tile;
        new_state
    }

    pub fn change_tile_ownership(
        &self,
        location: Location,
        new_owner: PlayerId,
        new_population: u16,
    ) -> Self {
        if Some(new_owner) == self.get_tile(location).owner {
            panic!("Changing ownership to the same player");
        }
        let mut new_state = self.clone();

        if let Some(prev_owner) = new_state.get_tile(location).owner {
            new_state.lands[prev_owner as usize] -= 1;
        }
        new_state.lands[new_owner as usize] += 1;

        let previous_tile = new_state.get_tile(location);
        let previous_owner = previous_tile.owner;

        let player_lost = previous_owner == Some(self.player_id);
        let player_won = new_owner == self.player_id;

        let new_type = if player_won {
            previous_tile.tile_type.own()
        } else {
            previous_tile.tile_type.lose()
        };

        let new_tile = Tile::new(new_type, new_population, Some(new_owner));
        new_state = new_state.update_tile(location, new_tile);

        // if lost, decrease mask around the tile by one, otherwise increase it
        let mask_delta = if player_lost { -1 } else { 1 };
        if player_lost || player_won {
            for x in location.0.saturating_sub(1)..=location.0 + 1 {
                for y in location.1.saturating_sub(1)..=location.1 + 1 {
                    if x < W && y < H {
                        new_state.fog_mask[x][y] =
                            (new_state.fog_mask[x][y] as i8 + mask_delta) as u8;

                        if new_state.fog_mask[x][y] == 0 {
                            new_state.tiles[x][y].tile_type =
                                new_state.tiles[x][y].tile_type.hide();
                        } else {
                            new_state.tiles[x][y].tile_type =
                                new_state.tiles[x][y].tile_type.reveal();
                        }
                    }
                }
            }
        }

        new_state
    }

    pub fn change_tile_population(&self, location: Location, pop_delta: i16) -> Self {
        let mut new_state = self.clone();
        let previous_tile = new_state.get_tile(location);

        if (previous_tile.population as i16).wrapping_add(pop_delta) as u16 > 65520 {
            panic!("Population overflow");
        }

        new_state = new_state.update_tile(
            location,
            Tile::new(
                previous_tile.tile_type,
                (previous_tile.population as i16).wrapping_add(pop_delta) as u16,
                previous_tile.owner,
            ),
        );
        new_state
    }

    pub fn process_command(
        &self,
        cmd: &MoveCommand,
        player_id: PlayerId,
        imagine_army: bool,
    ) -> Result<Self> {
        let mut new_state = self.clone();
        if imagine_army && new_state.get_tile(cmd.from).owner.is_none() {
            // make up an army of size players total population * 0.8
            let army_size = (self.armies[player_id as usize] as f32 * 0.8).round() as u16;
            new_state = new_state.update_tile(
                cmd.from,
                Tile::new(TileType::Enemy, army_size, Some(player_id)),
            );
        }

        let from_tile = new_state.get_tile(cmd.from);
        let to_tile = new_state.get_tile(cmd.to);

        if from_tile == to_tile {
            // this is a noop
            return Ok(new_state);
        }
        if self.turn == self.max_turn {
            // game is over
            return Ok(new_state);
        }

        let mut population_to_move = from_tile.population - 1;
        if cmd.half {
            population_to_move = population_to_move / 2;
        }

        // first, decrease the population of the tile we're moving from
        let mut new_state =
            new_state.change_tile_population(cmd.from, -(population_to_move as i16));

        if to_tile.owner == Some(player_id) {
            // if the tile is owned by the player, increase the population
            new_state = new_state.change_tile_population(cmd.to, population_to_move as i16);
        } else {
            let evaporated;
            // if the tile is not owned by the player, decrease the population
            if population_to_move > to_tile.population {
                // if the population is greater than the tile's population, change ownership
                new_state = new_state.change_tile_ownership(
                    cmd.to,
                    player_id,
                    population_to_move - to_tile.population,
                );

                evaporated = to_tile.population;

                // if this was a general, mark it as dead
                if matches!(
                    to_tile.tile_type,
                    TileType::EnemyGeneral | TileType::OwnedGeneral
                ) {
                    new_state.generals[to_tile.owner.unwrap() as usize] =
                        GeneralLocation::Dead(cmd.to);
                }

                // if command issuer is not player, and this is next to a friendly general, mark it as revealed
                let own_general = new_state.get_own_general();
                if player_id != self.player_id
                    && (cmd.to.0 as i32 - own_general.0 as i32).abs()
                        + (cmd.to.1 as i32 - own_general.1 as i32).abs()
                        <= 1
                {
                    new_state.general_revealed_to[player_id as usize] = true;
                }
            } else {
                // otherwise, just decrease the population
                new_state = new_state.change_tile_population(cmd.to, -(population_to_move as i16));
                evaporated = population_to_move;
            }

            // both players lose army
            new_state.armies[player_id as usize] -= evaporated;
            if let Some(owner) = to_tile.owner {
                new_state.armies[owner as usize] -= evaporated;
            }
        }

        Ok(new_state)
    }

    pub fn tick(&self, command: &MoveCommand) -> Result<Self> {
        let mut new_state = self.clone();
        new_state.turn += 1;

        // process command
        new_state = new_state.process_command(command, self.player_id, false)?;

        // increase population in all owned and enemy cities and generals
        // or all tiles if turn % 50 == 0
        for x in 0..W {
            for y in 0..H {
                let tile = new_state.get_tile((x, y));
                if tile.owner.is_some()
                    && ((matches!(
                        tile.tile_type,
                        TileType::OwnedCity
                            | TileType::OwnedGeneral
                            | TileType::EnemyCity
                            | TileType::EnemyGeneral
                    ) && new_state.turn % 2 == 0)
                        || new_state.turn % 50 == 0)
                {
                    new_state = new_state.change_tile_population((x, y), 1);
                }
            }
        }

        if new_state.turn % 2 == 0 && new_state.turn % 50 != 0 {
            // for every player increase population by their city count
            for player_id in 0..PLAYER_COUNT {
                new_state.armies[player_id] += new_state.city_count[player_id];
            }
        } else if new_state.turn % 50 == 0 {
            // for every player increase population by their land count
            for player_id in 0..PLAYER_COUNT {
                new_state.armies[player_id] += new_state.lands[player_id];
            }
        }

        Ok(new_state)
    }

    pub fn get_possible_commands(&self) -> Vec<MoveCommand> {
        let mut commands = Vec::with_capacity(self.lands[self.player_id as usize] as usize * 3);

        for x in 0..W {
            for y in 0..H {
                let tile = self.get_tile((x, y));
                if tile.owner == Some(self.player_id) && tile.population > 1 {
                    // if the tile is owned by the player and has population > 1
                    // add all possible commands from this tile
                    for (x2, y2) in get_neighbors((x, y), W, H) {
                        let tile2 = self.get_tile((x2, y2));
                        if tile2.tile_type.occupiable()
                            && (tile.population > tile2.population + 1
                                || tile2.owner == tile.owner
                                || tile.population > 5)
                        //         && (tile.population >= tile2.population))
                        //     )
                        {
                            commands.push((
                                tile.population + tile2.population,
                                MoveCommand {
                                    from: (x, y),
                                    to: (x2, y2),
                                    half: false,
                                },
                            ));
                        }
                    }
                }
            }
        }

        // we can always move "from general to general"
        // lets disable this for now, and only add it if theres nothing else to do
        if commands.is_empty() {
            commands.push((
                1,
                MoveCommand {
                    from: self.get_own_general(),
                    to: self.get_own_general(),
                    half: false,
                },
            ));
        }

        commands.sort_by(|a, b| b.0.cmp(&a.0));
        commands.into_iter().map(|(_, cmd)| cmd).collect()
    }

    pub fn reached_max_turns(&self) -> bool {
        self.turn >= self.max_turn
    }

    pub fn get_winner(&self) -> Option<PlayerId> {
        // find non-dead general
        let non_dead_general_idx: Vec<usize> = self
            .generals
            .iter()
            .enumerate()
            .filter(|(_, general)| !matches!(general, GeneralLocation::Dead(_)))
            .map(|(idx, _)| idx)
            .collect();

        if non_dead_general_idx.len() == 1 {
            Some(non_dead_general_idx[0] as PlayerId)
        } else {
            None
        }
    }
    pub fn player_id(&self) -> PlayerId {
        self.player_id
    }
    pub fn tiles(&self) -> &[[Tile; W]; H] {
        &self.tiles
    }
    pub fn generals(&self) -> &[GeneralLocation; PLAYER_COUNT] {
        &self.generals
    }

    pub fn get_score(&self, turn: &PlayerId) -> f64 {
        let mut winner_reward = 0.5;
        if let Some(winner) = self.get_winner() {
            if winner == *turn {
                winner_reward = 1f64;
            } else {
                winner_reward = 0f64;
            }
        }
        if *turn != self.player_id {
            return winner_reward * 50.;
        }
        // reward is army / sum_all_armies
        let sum_all_armies: u64 = self.armies.iter().map(|a| *a as u64).sum();
        let army_reward = self.armies[*turn as usize] as f64 / sum_all_armies as f64;

        // general reward is amount of enemy generals revealed
        let general_reward = self
            .generals
            .iter()
            .filter(|g| matches!(g, GeneralLocation::Known(_) | GeneralLocation::Dead(_)))
            .count() as f64
            - 1.;

        // general revealed punishment
        let general_revealed_punishment = (self.general_revealed_to.iter().filter(|g| **g).count()
            - 1) as f64
            / PLAYER_COUNT as f64;

        // reward for land / sum all owned lands
        let land_reward: f64 =
            self.lands[*turn as usize] as f64 / self.lands.iter().map(|v| *v as f64).sum::<f64>();

        let mut biggest_army_val = 1;
        let mut biggest_army_location = None;

        // having a big army with high manhattan distance from general is good
        // having big enemy army with low manhattan distance from general is bad
        let mut army_distance_reward = 1;
        let mut army_distance_punishment = 1;
        for x in 0..W {
            for y in 0..H {
                let tile: &Tile = self.get_tile((x, y));

                let distance = manhattan_distance((x, y), self.get_own_general());
                if tile.owner == Some(*turn) && tile.population > 3 && distance > 4 {
                    // army_distance_reward += (distance).pow(2) * tile.population as u64;
                }
                if tile.owner != Some(*turn)
                    && distance < 10
                    && tile.tile_type.occupiable()
                    && (tile.owner.is_some())
                // either opponent, or empty tile (excludes cities)
                {
                    army_distance_punishment += (10 - distance) * (tile.population + 1) as u64;
                }

                if tile.owner == Some(*turn) && tile.population > biggest_army_val {
                    biggest_army_val = tile.population;
                    biggest_army_location = Some((x, y));
                }
            }
        }
        // let army_distance_reward = if let Some(loc) = biggest_army_location {
        //     (manhattan_distance(self.get_own_general(), loc) as f64 * biggest_army_val as f64)
        // } else {
        //     0.
        // };
        // (army_distance_reward as f64 / self.armies[*turn as usize] as f64).log2();

        let army_distance_punishment =
            (army_distance_punishment as f64 / self.armies[*turn as usize] as f64);

        // max of army-land for all opponents
        let max_enemy_standing_army = (0..PLAYER_COUNT).fold(0, |max, player_id| {
            if player_id != *turn as usize {
                std::cmp::max(
                    max,
                    self.armies[player_id as usize] as i32 - self.lands[player_id as usize] as i32,
                )
            } else {
                max
            }
        }) as f64;

        // reward having a big army on general
        // let mut general_army_reward =
        //     self.get_tile(self.get_own_general()).population as f64 / max_enemy_standing_army;
        // if general_army_reward > 1. {
        //     general_army_reward = 1.;
        // }

        // sum all fog masks, divide by width * height
        let fog_reward = self
            .fog_mask
            .iter()
            .enumerate()
            .map(|(row, row_contents)| {
                row_contents
                    .iter()
                    .enumerate()
                    .map(|(col, v)| {
                        (min(*v, 1) as f64)
                            * if manhattan_distance(self.get_own_general(), (row, col)) < 15 {
                                1.
                            } else {
                                2. // double the reward for tiles far away from general
                            }
                    })
                    .sum::<f64>()
            })
            .sum::<f64>()
            / (W * H) as f64;

        debug!("state: {}", self);

        debug!(
            "winner_reward: {}, army_reward: {}, fog_reward: {}, land_reward: {}, general_reward: {}, general_revealed_punishment: {}, army_distance_reward: {}, army_distance_punishment: {}",
            winner_reward, army_reward, fog_reward, land_reward, general_reward, general_revealed_punishment, army_distance_reward, army_distance_punishment
        );

        winner_reward * 50. + army_reward * 20. + fog_reward * 5.
            - general_revealed_punishment * 10.
            + land_reward * 9.
            // + standing_army_reward * 1.
            // + army_distance_reward * 4.
            - army_distance_punishment * 3.
        // + general_army_reward * 3.
    }

    pub fn get_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

impl<const PLAYER_COUNT: usize, const W: usize, const H: usize> Hash
    for GameState<PLAYER_COUNT, W, H>
{
    fn hash<HS: Hasher>(&self, state: &mut HS) {
        self.turn.hash(state);
        self.tiles.hash(state);
        self.lands.hash(state);
        self.armies.hash(state);
        self.generals.hash(state);
        self.general_revealed_to.hash(state);
    }
}

impl Display for TileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TileType::VisibleEmpty => write!(f, " "),
            TileType::AssumedEmpty => write!(f, "?"),
            TileType::OwnedTile => write!(f, "x"),
            TileType::OwnedGeneral => write!(f, "X"),
            TileType::OwnedCity => write!(f, "C"),
            TileType::Enemy => write!(f, "o"),
            TileType::EnemyCity => write!(f, "E"),
            TileType::EnemyGeneral => write!(f, "O"),
            TileType::VisibleNeutralCity => write!(f, "c"),
            TileType::HiddenNeutralCity => write!(f, "c"),
            TileType::VisibleMountain => write!(f, "M"),
            TileType::HiddenObstacle => write!(f, "m"),
            TileType::Padding => write!(f, " "),
        }
    }
}

impl Display for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // if owned by self, print in green
        // if owned by enemy, print in red
        // if owned by neutral, print in yellow
        // if its Enemy or OwnedTile, print population

        let mut s = String::new();

        if self.tile_type.is_visible() {
            if self.tile_type.is_owned() {
                s.push_str("\x1b[32m");
            } else if self.tile_type.is_enemy() {
                s.push_str("\x1b[31m");
            } else {
                s.push_str("\x1b[33m");
            }

            if self.tile_type == TileType::OwnedTile || self.tile_type == TileType::Enemy {
                s.push_str(&format!("{: <2}", self.population));
            } else {
                s.push_str(&format!("{} ", self.tile_type));
            }

            s.push_str("\x1b[0m");
        } else {
            s.push_str(&format!("{} ", self.tile_type));
        }

        write!(f, "{}", s)
    }
}

impl<const PLAYER_COUNT: usize, const W: usize, const H: usize> Display
    for GameState<PLAYER_COUNT, W, H>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();

        // print turn, armies, lands
        s.push_str(&format!(
            "Turn: {}, Player: {}, Armies: {:?}, Lands: {:?}, Cities: {:?}\n",
            self.turn, self.player_id, self.armies, self.lands, self.city_count
        ));

        for y in 0..H {
            for x in 0..W {
                let tile = self.get_tile((x, y));
                s.push_str(&format!("{}", tile));
            }

            // now print tile types, regardless of visibility
            s.push_str("   ");
            for x in 0..W {
                let tile = self.get_tile((x, y));
                s.push_str(&format!("{} ", tile.tile_type));
            }

            // s.push_str("   ");
            // // now print populations, regardless of visibility
            // for x in 0..W {
            //     let tile = self.get_tile((x, y));
            //     s.push_str(&format!("{: <2}", tile.population));
            // }

            // now print fog mask with 3 spaces in between
            s.push_str("   ");
            for x in 0..W {
                s.push_str(&format!("{} ", self.fog_mask[x][y]));
            }
            s.push('\n');
        }

        // also print fog mask on a new line
        // s.push('\n');
        // for y in 0..H {
        //     for x in 0..W {
        //         s.push_str(&format!("{}", self.fog_mask[x][y]));
        //     }
        //     s.push('\n');
        // }

        write!(f, "{}", s)
    }
}

use noisy_float::prelude::{n64, Float};
use num_traits::{float::FloatConst, ToPrimitive, Zero};
use oxymcts::NodeMutRef;
use oxymcts::{
    uct_value, BackPropPolicy, DefaultBackProp, DefaultLazyTreePolicy, DefaultPlayout, Evaluator,
    GameTrait, LazyMcts, LazyMctsNode, LazyMctsTree, LazyTreePolicy, MctsNode, Nat, NodeId, Num,
    Playout, Tree,
};
use rand::{seq::SliceRandom, thread_rng, Rng};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Add, AddAssign, Div},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    constants::{MAX_TURNS, THREAD_COUNT},
    enemy::{possible_enemy_moves, EnemyMove},
    state::{GameState, GeneralsGameState, MoveCommand, PlayerId},
};

type OxyTree<State> = LazyMcts<
    State,
    DefaultLazyTreePolicy<State, GeneralsUctEvaluator, (), f64>,
    GeneralsPlayout,
    GeneralsBackProp,
    GeneralsUctEvaluator,
    (),
    f64,
>;

pub struct MctsTree {
    tree: OxyTree<GameStateWrapper>,
}
impl MctsTree {
    pub fn new(state: &GeneralsGameState) -> Self {
        let wrapped = GameStateWrapper {
            state: state.clone(),
            turn: state.player_id(),
        };

        Self {
            tree: OxyTree::new(wrapped),
        }
    }

    pub async fn train_until_interrupt(&self, interrupt: Arc<AtomicBool>) {
        let tree_ref = &self.tree;
        let c = f64::SQRT_2();
        let rollout_count = Arc::new(AtomicU32::new(0));

        let start = std::time::Instant::now();
        unsafe {
            // unsafe block because forgetting is unsafe, but we await it so its fine
            async_scoped::TokioScope::scope_and_collect(|s| {
                for _ in 0..THREAD_COUNT {
                    let proc = || async {
                        loop {
                            tree_ref.execute(&c, ());
                            rollout_count.fetch_add(1, Ordering::Relaxed);
                            if interrupt.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    };

                    s.spawn(proc());
                }
            })
            .await;
        }

        let end = start.elapsed();

        info!(
            "Rollout count: {}, rollout/s: {} for {}ms",
            rollout_count.load(Ordering::Relaxed),
            rollout_count.load(Ordering::Relaxed) as f64 / end.as_secs_f64(),
            end.as_millis()
        );
    }

    pub async fn get_best_move(&self, rollout_for: Duration) -> MoveCommand {
        let c = f64::SQRT_2();
        let notifier = Arc::new(AtomicBool::new(false));
        let interrupt = notifier.clone();

        tokio::spawn(async move {
            tokio::time::sleep(rollout_for).await;
            notifier.store(true, Ordering::Relaxed);
        });

        self.train_until_interrupt(interrupt).await;

        // info!("{}", tree.write_tree());

        let best_move = self.tree.best_move(&c);

        match best_move {
            CombinedMoveCommand::Friendly(m) => m,
            CombinedMoveCommand::Enemy(_) => panic!("Best move is enemy move??"),
        }
    }
}

#[derive(Debug, Clone, Hash)]
struct GameStateWrapper {
    state: GeneralsGameState,
    turn: PlayerId,
}

pub fn evaluate_state(state: GeneralsGameState) -> f64 {
    let player = state.player_id();
    GeneralsUctEvaluator::evaluate_leaf(
        GameStateWrapper {
            state,
            turn: 1 - player,
        },
        &player,
    )
}

#[derive(Debug, Clone, Hash)]
enum CombinedMoveCommand {
    Friendly(MoveCommand),
    Enemy(EnemyMove),
}

impl GameTrait for GameStateWrapper {
    type Player = PlayerId;

    type Move = CombinedMoveCommand;

    fn legals_moves(&self) -> Vec<Self::Move> {
        let moves = if self.turn == self.state.player_id() {
            self.state
                .get_possible_commands()
                .into_iter()
                .map(CombinedMoveCommand::Friendly)
                .collect()
        } else {
            possible_enemy_moves(&self.state, self.turn)
                .into_iter()
                .map(CombinedMoveCommand::Enemy)
                .collect()
        };

        // info!("Possible moves: {:?}", moves);
        moves
    }

    fn player_turn(&self) -> Self::Player {
        self.turn
    }

    fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.state.hash(&mut hasher);
        hasher.finish()
    }

    fn is_final(&self) -> bool {
        self.state.get_winner().is_some() || self.state.reached_max_turns()
    }

    fn do_move(&mut self, m: &Self::Move) {
        self.state = match m {
            CombinedMoveCommand::Friendly(m) => self.state.tick(m).unwrap(),
            CombinedMoveCommand::Enemy(m) => m.apply_on_state(&self.state, self.turn),
        };
        // debug assert that if state.player_id() == self.turn then we are friendly
        debug_assert!(
            (self.state.player_id() != self.turn) || matches!(m, CombinedMoveCommand::Friendly(_))
        );
        self.turn = (self.turn + 1) % 2;

        #[cfg(debug_assertions)]
        {
            debug!("Did move {:?}, new state\n{}", m, self.state)
        }
    }

    fn get_winner(&self) -> Self::Player {
        self.state.get_winner().expect("Game is not finished")
    }
}

pub struct GeneralsUctEvaluator;

impl Evaluator<GameStateWrapper, f64, ()> for GeneralsUctEvaluator {
    type Args = f64;
    type EvalResult = f64;

    fn eval_child(
        child: &LazyMctsNode<GameStateWrapper, f64, ()>,
        _turn: &PlayerId,
        parent_visits: Nat,
        &c: &Self::Args,
    ) -> Num {
        if child.n_visits == 0 {
            return n64(0f64);
        }
        uct_value(
            parent_visits,
            child.sum_rewards.to_f64().unwrap(),
            child.n_visits,
            c,
        )
    }

    fn evaluate_leaf(child: GameStateWrapper, turn: &PlayerId) -> Self::EvalResult {
        child.state.get_score(turn)
    }
}

struct GeneralsPlayout;
impl Playout<GameStateWrapper> for GeneralsPlayout {
    type Args = ();

    fn playout(mut state: GameStateWrapper, _args: ()) -> GameStateWrapper {
        // let friendly_move = state.player_turn() == state.state.player_id();

        while !state.is_final() {
            let moves = state.legals_moves();

            let m = if thread_rng().gen_range(0..10) < 4 {
                // pick randomly from first 10% of moves
                moves[0..((moves.len() / 10) + 1)]
                    .choose(&mut thread_rng())
                    .unwrap()
            } else {
                moves.choose(&mut thread_rng()).unwrap()
            };

            state.do_move(m);
        }
        state
    }
}

struct GeneralsBackProp;
impl<
        T: Clone,
        Move: Clone,
        R: Add + AddAssign + Div + Clone + Zero + ToPrimitive,
        A: Clone + Default,
    > BackPropPolicy<T, Move, R, A> for GeneralsBackProp
{
    fn backprop(tree: &Tree<MctsNode<T, Move, R, A>>, leaf: NodeId, reward: R) {
        let root_id = tree.root().id();
        let mut current_node_id = leaf;
        // Update the branch
        while current_node_id != root_id {
            let mut node_to_update = tree.get_mut(current_node_id).unwrap();
            node_to_update.value_mut().n_visits += 1;
            node_to_update.value_mut().sum_rewards =
                node_to_update.value().sum_rewards.clone() + reward.clone();
            current_node_id = node_to_update.parent_id();
        }
        // Update root
        let mut node_to_update = tree.get_mut(current_node_id).unwrap();
        node_to_update.value_mut().n_visits += 1;
        node_to_update.value_mut().sum_rewards += reward;
    }
}

struct GeneralsTreePolicy {}
impl GeneralsTreePolicy {
    pub fn select(
        tree: &LazyMctsTree<GameStateWrapper, f64, ()>,
        turn: &PlayerId,
        evaluator_args: f64,
    ) -> NodeId {
        let mut current_node_id = tree.root().id();
        while tree.get(current_node_id).unwrap().has_children() {
            if tree.get(current_node_id).unwrap().value().can_add_child() {
                return current_node_id;
            } else {
                current_node_id = Self::best_child(tree, turn, current_node_id, &evaluator_args);
            }
        }
        current_node_id
    }

    pub fn expand(
        mut node_to_expand: NodeMutRef<LazyMctsNode<GameStateWrapper, f64, ()>>,
        root_state: GameStateWrapper,
    ) -> (NodeId, GameStateWrapper) {
        let mut new_state = Self::update_state(root_state, &node_to_expand.value().state);
        if !node_to_expand.value().can_add_child() {
            return (node_to_expand.id(), new_state);
        }
        let mut unvisited_moves = &mut node_to_expand.value_mut().unvisited_moves;
        let index = thread_rng().gen_range(0..unvisited_moves.len());
        let move_to_expand = unvisited_moves[index].clone();
        unvisited_moves[index] = unvisited_moves.last().unwrap().clone();
        unvisited_moves.pop();

        let mut new_historic = node_to_expand.value().state.clone();
        new_state.do_move(&move_to_expand);
        new_historic.push(move_to_expand);

        let turn = new_state.turn;
        let new_node = MctsNode {
            sum_rewards: 0., //new_state.state.get_score(&turn),
            n_visits: 1,
            unvisited_moves: new_state.legals_moves(),
            hash: GameTrait::hash(&new_state),
            state: new_historic,
            additional_info: Default::default(),
        };

        (node_to_expand.add_child(new_node).id(), new_state)
    }
}

impl LazyTreePolicy<GameStateWrapper, GeneralsUctEvaluator, (), f64> for GeneralsTreePolicy {
    fn tree_policy(
        tree: &LazyMctsTree<GameStateWrapper, f64, ()>,
        root_state: GameStateWrapper,
        evaluator_args: &f64,
    ) -> (NodeId, GameStateWrapper) {
        let master_player = root_state.player_turn();
        let selected_node_id = Self::select(tree, &master_player, *evaluator_args);
        let node = tree.get_mut(selected_node_id).unwrap();
        Self::expand(node, root_state)
    }

    fn update_state(
        mut root_state: GameStateWrapper,
        historic: &[CombinedMoveCommand],
    ) -> GameStateWrapper {
        for m in historic {
            root_state.do_move(m)
        }
        root_state
    }

    fn best_child(
        tree: &LazyMctsTree<GameStateWrapper, f64, ()>,
        turn: &PlayerId,
        parent_id: NodeId,
        eval_args: &f64,
    ) -> NodeId {
        let parent_node = tree.get(parent_id).unwrap();
        let n_visits = parent_node.value().n_visits;

        let best = parent_node.get_best_child(|child| {
            GeneralsUctEvaluator::eval_child(child, turn, n_visits, eval_args)
        });

        best.unwrap()
    }
}

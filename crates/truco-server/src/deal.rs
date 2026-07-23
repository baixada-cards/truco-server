use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use truco_engine::{Card, HandStart, Hands, Player, Rank, Suit, Turnup};

pub(crate) fn random_hand_start(rng: &mut StdRng) -> HandStart {
    let mut deck = full_deck_faces();
    deck.shuffle(rng);

    let turnup_face = deck[6];
    let turnup = Turnup {
        rank: turnup_face.0,
        suit: turnup_face.1,
    };

    let zero = cards_with_generated_ids(0, vec![deck[0], deck[1], deck[2]], &turnup);
    let one = cards_with_generated_ids(1, vec![deck[3], deck[4], deck[5]], &turnup);

    HandStart {
        turnup,
        hands: Hands {
            zero: zero.into(),
            one: one.into(),
        },
    }
}

/// Like [`random_hand_start`], but the turnup is forced to the given rank
/// (uniform over its suits; hands dealt uniformly from the rest). Dev tooling
/// only: makes the locally shipped study turn-up class reproducible between
/// hands.
pub(crate) fn random_hand_start_with_vira(rng: &mut StdRng, vira_rank: Rank) -> HandStart {
    let mut deck = full_deck_faces();
    deck.shuffle(rng);
    let turnup_index = deck
        .iter()
        .position(|(rank, _)| *rank == vira_rank)
        .expect("every rank has four copies in the deck");
    let turnup_face = deck.remove(turnup_index);
    let turnup = Turnup {
        rank: turnup_face.0,
        suit: turnup_face.1,
    };

    let zero = cards_with_generated_ids(0, vec![deck[0], deck[1], deck[2]], &turnup);
    let one = cards_with_generated_ids(1, vec![deck[3], deck[4], deck[5]], &turnup);

    HandStart {
        turnup,
        hands: Hands {
            zero: zero.into(),
            one: one.into(),
        },
    }
}

pub(crate) fn full_deck_faces() -> Vec<(Rank, Suit)> {
    let ranks = [
        Rank::Four,
        Rank::Five,
        Rank::Six,
        Rank::Seven,
        Rank::Queen,
        Rank::Jack,
        Rank::King,
        Rank::Ace,
        Rank::Two,
        Rank::Three,
    ];
    let suits = [Suit::Diamonds, Suit::Spades, Suit::Hearts, Suit::Clubs];
    let mut deck = Vec::new();
    for rank in ranks {
        for suit in suits {
            deck.push((rank, suit));
        }
    }
    deck
}

pub(crate) fn cards_with_generated_ids(
    player: Player,
    mut cards: Vec<(Rank, Suit)>,
    turnup: &Turnup,
) -> Vec<Card> {
    cards.sort_by_key(|(rank, suit)| strength_key(*rank, *suit, turnup));
    cards
        .into_iter()
        .enumerate()
        .map(|(index, (rank, suit))| Card {
            id: format!("p{player}c{index}").into(),
            rank,
            suit,
        })
        .collect()
}

pub(crate) fn strength_key(rank: Rank, suit: Suit, turnup: &Turnup) -> u8 {
    if rank == turnup.rank.next_for_manilha() {
        10 + suit.manilha_strength() as u8
    } else {
        rank.index() as u8
    }
}

pub(crate) fn seeded_rng(seed: Option<u64>, salt: u64) -> StdRng {
    let derived = seed.unwrap_or_else(rand::random::<u64>) ^ salt;
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&derived.to_le_bytes());
    StdRng::from_seed(bytes)
}

pub(crate) fn other_player(player: Player) -> Player {
    match player {
        0 => 1,
        _ => 0,
    }
}

use super::types::LoadingKind;

const PLAUSIBLE: &[&str] = &[
    "Getting situated…",
    "Sorting things out…",
    "Putting things together…",
    "Making arrangements…",
    "Finding everything…",
    "Gathering things…",
    "Getting organized…",
    "Waking things up…",
    "Bringing things online…",
    "Preparing the room…",
    "Setting the table…",
    "Opening the curtains…",
    "Turning the lights on…",
    "Looking around…",
    "Getting acquainted…",
    "Checking the premises…",
    "Dusting things off…",
    "Putting everything in order…",
    "Making things presentable…",
    "Getting things ready…",
    "Fetching things…",
    "Unpacking things…",
    "Laying things out…",
    "Finding its bearings…",
    "Checking its notes…",
    "Consulting the records…",
    "Reviewing the situation…",
    "Pulling some strings…",
    "Handling paperwork…",
    "Moving some papers around…",
    "Locating the relevant department…",
    "Moving things along…",
    "Engaging mechanisms…",
    "Restoring decorum…",
    "Maintaining appearances…",
];

const WHIMSICAL: &[&str] = &[
    "Frobnicking…",
    "Faffing…",
    "Finagling…",
    "Fandangling…",
    "Wibbling…",
    "Percolating…",
    "Cogitating…",
    "Conjuring…",
    "Scurrying…",
    "Rummaging…",
];

const UNHINGED: &[&str] = &[
    "Performing administrative rituals…",
    "Checking behind the curtain…",
    "Opening, in principle…",
    "Appeasing the machinery…",
    "Inventing the relevant department…",
];

pub(super) fn loading_message(kind: LoadingKind, seed: u64, rotation: u64) -> &'static str {
    let kind_salt = match kind {
        LoadingKind::Opening => 0xA076_1D64_78BD_642F,
        LoadingKind::Resuming => 0xE703_7ED1_A0B4_28DB,
    };
    let count = PLAUSIBLE.len() + WHIMSICAL.len() + UNHINGED.len();
    let index = (mix(seed ^ kind_salt) as usize + rotation as usize * 17) % count;
    if index < PLAUSIBLE.len() {
        PLAUSIBLE[index]
    } else if index < PLAUSIBLE.len() + WHIMSICAL.len() {
        WHIMSICAL[index - PLAUSIBLE.len()]
    } else {
        UNHINGED[index - PLAUSIBLE.len() - WHIMSICAL.len()]
    }
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_messages_do_not_repeat_consecutively() {
        for kind in [LoadingKind::Opening, LoadingKind::Resuming] {
            for seed in 0..50 {
                for rotation in 1..20 {
                    assert_ne!(
                        loading_message(kind, seed, rotation - 1),
                        loading_message(kind, seed, rotation)
                    );
                }
            }
        }
    }

    #[test]
    fn category_weighting_is_seventy_twenty_ten() {
        assert_eq!(PLAUSIBLE.len(), 35);
        assert_eq!(WHIMSICAL.len(), 10);
        assert_eq!(UNHINGED.len(), 5);
    }
}

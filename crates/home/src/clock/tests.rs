use core::time::Duration;

use super::{UnixMillis, UnixSecs};

#[test]
fn a_millisecond_clock_reads_as_its_whole_seconds() {
    assert_eq!(
        [
            UnixMillis(0).secs(),
            UnixMillis(1_999).secs(),
            UnixMillis(2_000).secs()
        ],
        [UnixSecs(0), UnixSecs(1), UnixSecs(2)]
    );
}

#[test]
fn a_second_clock_reads_as_milliseconds_and_saturates() {
    assert_eq!(
        [UnixSecs(3).millis(), UnixSecs(u64::MAX).millis()],
        [UnixMillis(3_000), UnixMillis(u64::MAX)]
    );
}

#[test]
fn a_clock_is_read_from_the_time_since_the_epoch() {
    let since = Duration::from_millis(1_795_348_800_250);
    assert_eq!(
        (UnixSecs::since_epoch(since), UnixMillis::since_epoch(since)),
        (UnixSecs(1_795_348_800), UnixMillis(1_795_348_800_250))
    );
    assert_eq!(UnixMillis::since_epoch(Duration::MAX), UnixMillis(u64::MAX));
}

#[test]
fn the_time_since_an_earlier_second_is_never_negative() {
    assert_eq!(
        [
            UnixSecs(130).elapsed_since(UnixSecs(100)),
            UnixSecs(50).elapsed_since(UnixSecs(100)),
        ],
        [Duration::from_secs(30), Duration::ZERO]
    );
}

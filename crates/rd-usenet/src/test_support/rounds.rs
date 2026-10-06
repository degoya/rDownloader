//! The order of commands and answers at the fixture, one sequence over every connection
//! (RD-1120-09).
//!
//! A wall-clock bound on idle time fails whenever a loaded runner is slow enough, however wide
//! the bound is drawn. The order does not depend on the runner: a connection that sat out a
//! whole round trip of another one - a command that arrived after it fell idle and was answered
//! before it was busy again - waited for the pipeline, not for the scheduler.

use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Rounds {
    tick: u64,
    lines: Vec<Line>,
    /// Every answered command as `(connection, arrived, answered)`.
    trips: Vec<(usize, u64, u64)>,
    last_command: Option<u64>,
}

/// One connection: its commands still waiting for an answer, and when it had none.
#[derive(Default)]
struct Line {
    waiting: VecDeque<u64>,
    idle_since: Option<u64>,
    idle: Vec<(u64, u64)>,
}

impl Rounds {
    pub(super) fn connected(&mut self) {
        self.lines.push(Line::default());
    }

    pub(super) fn command(&mut self, index: usize) {
        self.tick += 1;
        let tick = self.tick;
        let line = &mut self.lines[index];
        if line.waiting.is_empty()
            && let Some(since) = line.idle_since.take()
        {
            line.idle.push((since, tick));
        }
        line.waiting.push_back(tick);
        self.last_command = Some(tick);
    }

    pub(super) fn answered(&mut self, index: usize) {
        self.tick += 1;
        let tick = self.tick;
        let line = &mut self.lines[index];
        if let Some(arrived) = line.waiting.pop_front() {
            self.trips.push((index, arrived, tick));
        }
        if line.waiting.is_empty() {
            line.idle_since = Some(tick);
        }
    }

    /// Per connection, how many round trips of the other connections it sat out idle while
    /// some article was still to be asked for: up to the last command the fixture received.
    /// Counted from a connection's first answer, so opening it is not idle time.
    pub(super) fn sat_out(&self) -> Vec<usize> {
        let Some(last) = self.last_command else {
            return Vec::new();
        };
        self.lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let open = line.idle_since.map(|since| (since, last));
                line.idle
                    .iter()
                    .copied()
                    .chain(open)
                    .map(|(begin, end)| {
                        self.trips
                            .iter()
                            .filter(|(other, arrived, answered)| {
                                *other != index && begin < *arrived && *answered < end
                            })
                            .count()
                    })
                    .sum()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Rounds;

    #[test]
    fn a_line_that_waits_out_another_lines_round_trip_is_counted() {
        let mut rounds = Rounds::default();
        rounds.connected();
        rounds.connected();
        rounds.command(0);
        rounds.answered(0);
        // Line 0 is idle; line 1 is asked and answered before line 0 is asked again.
        rounds.command(1);
        rounds.answered(1);
        rounds.command(0);
        assert_eq!(rounds.sat_out(), vec![1, 0]);
    }

    #[test]
    fn a_line_asked_again_before_the_other_answers_sat_nothing_out() {
        let mut rounds = Rounds::default();
        rounds.connected();
        rounds.connected();
        rounds.command(0);
        rounds.command(1);
        rounds.answered(0);
        rounds.command(0);
        rounds.answered(1);
        rounds.answered(0);
        assert_eq!(rounds.sat_out(), vec![0, 0]);
    }
}

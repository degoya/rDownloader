//! The server of RD-108-27, and the client that no longer takes its word for it.
//!
//! The field: after RD-108-25 put two `BODY` commands on every connection, a live provider
//! answered them out of step - the earlier request on a line read the later one's body, ten
//! articles ahead at ten connections - and the assembly failed the file with
//! `non-contiguous yEnc part range`, twenty in-flight articles thrown away per attempt. The
//! client attributed answers to commands by order alone. RFC 3977 puts the message-id on
//! every `222` line; that is the check.
//!
//! The fixture plays that server with `swaps_pipelined_answers`, and the in-order server it
//! was believed to be for the counter-check.

use rd_core::NzbSegmentState;

use crate::test_support::{FixtureBehaviour, article_set, completed, run, states};

/// The field failure, reproduced: one connection, two commands on it, the server answers
/// the second first. Before RD-108-27 the first request took the second's body and the
/// assembly failed with `non-contiguous yEnc part range 3001-6000`. Now the `222` line
/// gives the answer away, the line is dropped, both articles are fetched again on a fresh
/// connection one at a time, and the file is the file.
#[tokio::test]
async fn the_earlier_request_on_a_line_no_longer_takes_the_later_ones_body() {
    let set = article_set(5, &[]);
    let run = run(
        &set,
        1,
        FixtureBehaviour {
            names_message_id: true,
            spells_id_differently: false,
            swaps_pipelined_answers: true,
            ..FixtureBehaviour::default()
        },
    )
    .await;
    let (path, missing) = completed(run.outcome);
    assert!(
        run.log.swapped() >= 1,
        "the fixture never had two commands queued at once, so nothing was swapped"
    );
    assert_eq!(missing, 0);
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(
        written, set.expected,
        "the assembled bytes are the articles in order"
    );
    assert_eq!(
        run.pool.pipeline_depth(0),
        1,
        "the server is down to one command per connection"
    );
    assert_eq!(
        run.log.connection_count(),
        2,
        "the out-of-step line was dropped and one fresh connection carried the rest"
    );
    let connections = run.log.connections.lock().expect("fixture log").clone();
    assert_eq!(
        connections[0],
        ["part-1@example.test", "part-2@example.test"],
        "the first line carried the pipelined pair that came back swapped"
    );
}

/// The other shape a foreign id can take: the server names the requested article, but not
/// the way the client spelt it. That is nothing to break a line over - nothing was swapped,
/// the body is the right one - and before this case existed it cost every article of every
/// file: three tries, then a hole. The server is unverifiable, so it gets one command per
/// connection, and keeps its line and every article.
#[tokio::test]
async fn a_server_that_spells_the_id_differently_keeps_every_article() {
    let set = article_set(5, &[]);
    let run = run(
        &set,
        1,
        FixtureBehaviour {
            names_message_id: true,
            spells_id_differently: true,
            swaps_pipelined_answers: false,
            ..FixtureBehaviour::default()
        },
    )
    .await;
    let (path, missing) = completed(run.outcome);
    assert_eq!(missing, 0, "no article was given up on");
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(written, set.expected);
    assert_eq!(
        run.pool.pipeline_depth(0),
        1,
        "unverifiable, so one at a time"
    );
    assert_eq!(run.log.connection_count(), 1, "the line was kept");
    assert_eq!(
        run.log.requests().len(),
        5,
        "every article was asked for exactly once"
    );
}

/// A refusal has no message-id to check. Read beside another request, it may be the other
/// request's `430`: here the server answers the pair (present, missing) as (430, body), and
/// before RD-108-27 the present article was recorded as missing and the missing one's row
/// was checkpointed with the present one's bytes. Now the refusal is asked again alone
/// before it is believed, and the hole ends up where the hole is.
#[tokio::test]
async fn a_refusal_read_beside_another_request_is_believed_only_once_asked_alone() {
    let set = article_set(3, &[2]);
    let run = run(
        &set,
        1,
        FixtureBehaviour {
            names_message_id: true,
            spells_id_differently: false,
            swaps_pipelined_answers: true,
            ..FixtureBehaviour::default()
        },
    )
    .await;
    let (path, missing) = completed(run.outcome);
    assert!(
        run.log.swapped() >= 1,
        "the fixture never had two commands queued at once, so nothing was swapped"
    );
    assert_eq!(missing, 1);
    assert_eq!(
        states(&run.database, &run.file).await,
        [
            NzbSegmentState::Completed,
            NzbSegmentState::Failed,
            NzbSegmentState::Completed
        ],
        "the missing segment is the one the server has not got"
    );
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(
        written, set.expected,
        "the zero hole sits at the missing article"
    );
    let requests = run.log.requests();
    assert!(
        requests
            .iter()
            .filter(|id| id.as_str() == "part-2@example.test")
            .count()
            >= 2,
        "the refusal was asked once more on a line of its own: {requests:?}"
    );
}

/// The counter-check for the two suspicions the evidence started with: against a server
/// that answers in order, ten connections with two commands each and a `430` on every
/// seventh article keep the client in step - no line is dropped, pipelining stays on, every
/// hole is a missing article and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_order_server_that_refuses_some_articles_keeps_the_client_in_step() {
    let missing = [7, 14, 21, 28, 35, 42, 49, 56];
    let set = article_set(60, &missing);
    let run = run(&set, 10, FixtureBehaviour::default()).await;
    let (path, holes) = completed(run.outcome);
    assert_eq!(holes, missing.len());
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(written, set.expected);
    assert_eq!(
        run.pool.pipeline_depth(0),
        crate::pool::PIPELINE_DEPTH,
        "pipelining was never given up"
    );
    assert_eq!(
        run.log.connection_count(),
        10,
        "no line was dropped: every connection is one the pool opened for the limit"
    );
    let states = states(&run.database, &run.file).await;
    for (index, state) in states.iter().enumerate() {
        let expected = if missing.contains(&(index + 1)) {
            NzbSegmentState::Failed
        } else {
            NzbSegmentState::Completed
        };
        assert_eq!(*state, expected, "segment {}", index + 1);
    }
}

/// A `222` line without a message-id gives the client nothing to check an answer against,
/// so that server gets one command per connection - the wire shape before RD-108-25 - and
/// keeps every article.
#[tokio::test]
async fn a_server_that_names_no_message_id_gets_one_command_per_connection() {
    let set = article_set(4, &[]);
    let run = run(
        &set,
        1,
        FixtureBehaviour {
            names_message_id: false,
            spells_id_differently: false,
            swaps_pipelined_answers: false,
            ..FixtureBehaviour::default()
        },
    )
    .await;
    let (path, missing) = completed(run.outcome);
    assert_eq!(missing, 0);
    let written = tokio::fs::read(&path).await.expect("assembled file");
    assert_eq!(written, set.expected);
    assert_eq!(run.pool.pipeline_depth(0), 1);
    assert_eq!(run.log.connection_count(), 1, "the line itself was kept");
}

//! Cutting a script into what each server runs, and telling reads from
//! writes.
//!
//! One small lexer serves both: it knows where strings, quoted names and
//! comments start and end, so nothing inside one is ever taken for a `GO`, a
//! `;`, a `/` or a keyword.
//!
//! SQL Server runs a batch at a time, so a script is cut only at `GO` lines
//! and each batch goes whole: `declare @x`, a blank line and `select @x`
//! stay together. (sql-bench also cut at blank lines, which broke exactly
//! that.) Oracle runs one statement per call, so a script is cut at every
//! `;` and at `/` lines, except inside PL/SQL, which runs to the `end;` that
//! closes it or to the next `/` line. Postgres is cut at every `;` as psql
//! cuts it: a function body is a `$$` string, and only a SQL-standard body
//! (`begin atomic … end`) holds `;`s outside one.

use std::ops::Range;

use crate::config::Kind;

/// One piece of a script that is not whitespace or a comment.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// A run of name characters: a keyword, a name, a number.
    Word(Range<usize>),
    /// A string or a quoted name, which is code but never a keyword.
    Quoted(Range<usize>),
    Symbol(usize, char),
    /// A line holding only `GO` (SQL Server) or `/` (Oracle).
    Separator(Range<usize>),
}

impl Token {
    fn start(&self) -> usize {
        match self {
            Self::Word(range) | Self::Quoted(range) | Self::Separator(range) => range.start,
            Self::Symbol(at, _) => *at,
        }
    }
}

/// Name characters: T-SQL's `@var` and `#temp`, Oracle's `$` and `#`, and
/// anything non-ASCII, so a run never splits a character.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#' | b'@') || byte >= 0x80
}

fn tokens(text: &str, kind: Kind) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    let mut line_start = true;
    while at < bytes.len() {
        if line_start {
            line_start = false;
            let end = text[at..]
                .find('\n')
                .map_or(text.len(), |offset| at + offset);
            let line = text[at..end].trim();
            let separator = match kind {
                Kind::Mssql => line.eq_ignore_ascii_case("go"),
                Kind::Oracle => line == "/",
                Kind::Postgres => false,
            };
            if separator {
                out.push(Token::Separator(at..end));
                at = end;
                continue;
            }
        }
        let byte = bytes[at];
        let next = bytes.get(at + 1).copied();
        match byte {
            b'\n' => {
                line_start = true;
                at += 1;
            }
            _ if byte.is_ascii_whitespace() => at += 1,
            // Up to the newline, not past it, so the next line is still seen
            // starting.
            b'-' if next == Some(b'-') => {
                at = text[at..]
                    .find('\n')
                    .map_or(text.len(), |offset| at + offset);
            }
            b'/' if next == Some(b'*') => at = comment_end(text, at, kind),
            b'\'' | b'"' => {
                let end = quoted_end(bytes, at + 1, byte);
                out.push(Token::Quoted(at..end));
                at = end;
            }
            b'[' if kind == Kind::Mssql => {
                let end = quoted_end(bytes, at + 1, b']');
                out.push(Token::Quoted(at..end));
                at = end;
            }
            // `$$…$$` and `$body$…$body$`; `$1` is a parameter.
            b'$' if kind == Kind::Postgres && dollar_tag(text, at).is_some() => {
                let end = dollar_quote_end(text, at);
                out.push(Token::Quoted(at..end));
                at = end;
            }
            _ if is_word_byte(byte) => {
                let start = at;
                while at < bytes.len() && is_word_byte(bytes[at]) {
                    at += 1;
                }
                // `N'…'`, and Oracle's `q'[…]'` and `nq'…'`: the letters
                // belong to the string.
                if bytes.get(at) == Some(&b'\'') {
                    let word = &text[start..at];
                    if kind == Kind::Oracle
                        && (word.eq_ignore_ascii_case("q") || word.eq_ignore_ascii_case("nq"))
                    {
                        let end = q_quote_end(text, at + 1);
                        out.push(Token::Quoted(start..end));
                        at = end;
                        continue;
                    }
                    if word.eq_ignore_ascii_case("n") {
                        let end = quoted_end(bytes, at + 1, b'\'');
                        out.push(Token::Quoted(start..end));
                        at = end;
                        continue;
                    }
                    // Postgres's `E'it\'s'`, where a backslash escapes.
                    if kind == Kind::Postgres && word.eq_ignore_ascii_case("e") {
                        let end = escaped_end(bytes, at + 1);
                        out.push(Token::Quoted(start..end));
                        at = end;
                        continue;
                    }
                }
                out.push(Token::Word(start..at));
            }
            _ => {
                let symbol = text[at..].chars().next().unwrap_or(' ');
                out.push(Token::Symbol(at, symbol));
                at += symbol.len_utf8();
            }
        }
    }
    out
}

/// Past the `close` that ends a quote begun before `from`; a doubled one is
/// the quote's own (`'it''s'`, `[a]]b]`). An unclosed quote runs to the end.
fn quoted_end(bytes: &[u8], mut from: usize, close: u8) -> usize {
    while from < bytes.len() {
        if bytes[from] == close {
            if bytes.get(from + 1) == Some(&close) {
                from += 2;
                continue;
            }
            return from + 1;
        }
        from += 1;
    }
    bytes.len()
}

/// Past the `*/` that closes the comment at `at`. T-SQL's and Postgres's
/// comments nest; Oracle's do not.
fn comment_end(text: &str, at: usize, kind: Kind) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut index = at;
    while index + 1 < bytes.len() {
        match (bytes[index], bytes[index + 1]) {
            (b'/', b'*') if depth == 0 || kind != Kind::Oracle => {
                depth += 1;
                index += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                index += 2;
                if depth == 0 {
                    return index;
                }
            }
            _ => index += 1,
        }
    }
    text.len()
}

/// Past an Oracle `q'X…X'` whose delimiter starts at `from`: brackets close
/// with their partner, anything else with itself.
fn q_quote_end(text: &str, from: usize) -> usize {
    let Some(open) = text[from..].chars().next() else {
        return text.len();
    };
    let close = match open {
        '[' => ']',
        '(' => ')',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let body = from + open.len_utf8();
    let end = format!("{close}'");
    text[body..]
        .find(&end)
        .map_or(text.len(), |offset| body + offset + end.len())
}

/// The `$tag$` that opens a Postgres dollar quote at `at`: `$`, a tag that
/// does not start with a digit, `$`.
fn dollar_tag(text: &str, at: usize) -> Option<&str> {
    let rest = &text[at + 1..];
    let close = rest.find('$')?;
    let tag = &rest[..close];
    let word = |character: char| character.is_alphanumeric() || character == '_';
    (!tag.starts_with(|first: char| first.is_ascii_digit()) && tag.chars().all(word))
        .then(|| &text[at..at + close + 2])
}

/// Past the `$tag$` that closes the dollar quote opened at `at`; an unclosed
/// one runs to the end.
fn dollar_quote_end(text: &str, at: usize) -> usize {
    let Some(tag) = dollar_tag(text, at) else {
        return text.len();
    };
    let body = at + tag.len();
    text[body..]
        .find(tag)
        .map_or(text.len(), |offset| body + offset + tag.len())
}

/// Past the quote that ends an `E'…'` string begun before `from`, where `\`
/// escapes the next byte and a doubled quote is the string's own.
fn escaped_end(bytes: &[u8], mut from: usize) -> usize {
    while from < bytes.len() {
        match bytes[from] {
            b'\\' => from += 2,
            b'\'' if bytes.get(from + 1) == Some(&b'\'') => from += 2,
            b'\'' => return from + 1,
            _ => from += 1,
        }
    }
    bytes.len()
}

/// A statement ready to send, and whether it may change anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub sql: String,
    pub writes: bool,
}

/// Every statement in `text`, in order, trimmed and never empty, each
/// classified.
pub fn statements(text: &str, kind: Kind) -> Vec<Statement> {
    split(text, kind)
        .into_iter()
        .map(|sql| Statement {
            writes: writes(&sql, kind),
            sql,
        })
        .collect()
}

fn split(text: &str, kind: Kind) -> Vec<String> {
    let tokens = tokens(text, kind);
    let word = |range: &Range<usize>| text[range.clone()].to_ascii_lowercase();
    let mut out = Vec::new();
    let mut push = |range: Range<usize>| {
        let piece = text[range].trim();
        if !piece.is_empty() {
            out.push(piece.to_owned());
        }
    };
    // Where the statement being read starts, at its first token.
    let mut start: Option<usize> = None;
    // Set while that statement is PL/SQL.
    let mut block: Option<Block> = None;
    // Set while that statement creates a Postgres function or procedure: how
    // deep in a `begin atomic` body it is.
    let mut atomic: Option<usize> = None;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Separator(range) => {
                if let Some(from) = start.take() {
                    push(from..range.start);
                }
                block = None;
            }
            // SQL Server hands the whole batch over; its `;`s are its own.
            Token::Symbol(at, ';') if kind == Kind::Oracle => match &mut block {
                // The `;` ends the statement and is not part of it: Oracle
                // says ORA-00933 to one before 23ai.
                None => {
                    if let Some(from) = start.take() {
                        push(from..*at);
                    }
                }
                // PL/SQL keeps it: `end;` is the language, not punctuation.
                Some(unit) if unit.closed => {
                    if let Some(from) = start.take() {
                        push(from..*at + 1);
                    }
                    block = None;
                }
                Some(unit) => unit.header = false,
            },
            Token::Symbol(at, ';') if kind == Kind::Postgres => {
                if atomic.is_none_or(|depth| depth == 0)
                    && let Some(from) = start.take()
                {
                    push(from..*at);
                    atomic = None;
                }
            }
            Token::Symbol(_, ';') => {}
            token => {
                if start.is_none() {
                    start = Some(token.start());
                    block = (kind == Kind::Oracle && is_plsql(text, &tokens[index..]))
                        .then(Block::default);
                    atomic =
                        (kind == Kind::Postgres && is_routine(text, &tokens[index..])).then_some(0);
                }
                // psql's rule: in a routine, `begin` and `case` open and
                // `end` closes, and a `;` inside counts for nothing.
                if let (Some(depth), Token::Word(range)) = (&mut atomic, token) {
                    match word(range).as_str() {
                        "begin" => *depth += 1,
                        "case" if *depth > 0 => *depth += 1,
                        "end" if *depth > 0 => *depth -= 1,
                        _ => {}
                    }
                }
                if let (Some(unit), Token::Word(range)) = (&mut block, token) {
                    // Only the very next token: `end; if` is not `end if`.
                    let next = match tokens.get(index + 1) {
                        Some(Token::Word(range)) => Some(word(range)),
                        _ => None,
                    };
                    unit.see(&word(range), next.as_deref());
                }
            }
        }
    }
    if let Some(from) = start {
        push(from..text.len());
    }
    out
}

/// Whether the statement starting at `tokens[0]` is PL/SQL: a block, or the
/// `create` of a stored program, whose body is full of `;`.
fn is_plsql(text: &str, tokens: &[Token]) -> bool {
    let mut words = tokens.iter().filter_map(|token| match token {
        Token::Word(range) => Some(text[range.clone()].to_ascii_lowercase()),
        _ => None,
    });
    match words.next().as_deref() {
        Some("begin" | "declare") => true,
        Some("create") => {
            let mut words = words.skip_while(|word| {
                ["or", "replace", "editionable", "noneditionable"].contains(&word.as_str())
            });
            match words.next().as_deref() {
                Some("procedure" | "function" | "package" | "trigger") => true,
                Some("type") => words.next().as_deref() == Some("body"),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Whether the statement starting at `tokens[0]` creates a Postgres function
/// or procedure, whose body may be `begin atomic … end`.
fn is_routine(text: &str, tokens: &[Token]) -> bool {
    let mut words = tokens.iter().filter_map(|token| match token {
        Token::Word(range) => Some(text[range.clone()].to_ascii_lowercase()),
        _ => None,
    });
    words.next().as_deref() == Some("create")
        && matches!(
            words
                .find(|word| !["or", "replace"].contains(&word.as_str()))
                .as_deref(),
            Some("function" | "procedure")
        )
}

/// Where a PL/SQL unit is in its nesting, word by word.
///
/// A frame opens at `begin` and at `case`, and closes at `end`, except `end
/// if` and `end loop`, whose openers never opened one. `declare`, and the
/// `is`/`as` after a procedure, function, package or type body header, open
/// a frame that is waiting for its `begin`, which then joins it rather than
/// opening another: a package has no `begin`, a procedure has one, and both
/// have one `end`. The unit is over at the `;` after the `end` that closes
/// its last frame.
// ponytail: no parser, so a compound trigger or a Java call spec (`as
// language java …;`) never looks closed and runs to the next `/` line or the
// end of the script, which is where SQL*Plus would end it too.
#[derive(Default)]
struct Block {
    open: Vec<bool>,
    /// A procedure, function, package or type body header waits for its `is`.
    header: bool,
    closed: bool,
    /// The word after `end` that is part of it (`if`, `loop`, `case`).
    skip: bool,
    previous: String,
}

impl Block {
    fn see(&mut self, word: &str, next: Option<&str>) {
        if std::mem::take(&mut self.skip) {
            self.previous = word.to_owned();
            return;
        }
        match word {
            "procedure" | "function" | "package" => self.header = true,
            "body" if self.previous == "type" => self.header = true,
            "is" | "as" if self.header => {
                self.header = false;
                self.open.push(false);
            }
            "declare" => self.open.push(false),
            "begin" => match self.open.last_mut() {
                Some(waiting @ false) => *waiting = true,
                _ => self.open.push(true),
            },
            "case" => self.open.push(true),
            "end" => {
                self.skip = matches!(next, Some("if" | "loop" | "case"));
                if next != Some("if") && next != Some("loop") {
                    self.open.pop();
                    self.closed = self.open.is_empty();
                }
            }
            _ => {}
        }
        self.previous = word.to_owned();
    }
}

/// Words that make a statement a write wherever they appear outside a
/// string, a quoted name or a comment. A leading `select` or `with` is
/// needed as well, so these catch the rest: `select … into`, `with … delete`,
/// `select … for update`, a second statement in a SQL Server batch, `exec`,
/// the lock hints, and `nextval`.
const WRITE_WORDS: &[&str] = &[
    "insert",
    "update",
    "delete",
    "merge",
    "into",
    "create",
    "alter",
    "drop",
    "truncate",
    "exec",
    "execute",
    "call",
    "grant",
    "revoke",
    "deny",
    "declare",
    "set",
    "begin",
    "commit",
    "rollback",
    "use",
    "dbcc",
    "backup",
    "restore",
    "kill",
    "shutdown",
    "reconfigure",
    "bulk",
    "openquery",
    "openrowset",
    "opendatasource",
    "nextval",
    "updlock",
    "xlock",
];

/// Postgres functions a select can call that change something: a sequence,
/// a setting, another session, a file on the server, another database.
const POSTGRES_WRITE_WORDS: &[&str] = &[
    "setval",
    "set_config",
    "pg_cancel_backend",
    "pg_terminate_backend",
    "lo_import",
    "lo_export",
    "dblink_exec",
];

/// Whether `sql` may change anything. Conservative: it is a read only when
/// its first word is `select` or `with` (on Postgres also `show`, `table`,
/// `values` and `explain`, whose `analyze` runs what follows it) and no word
/// in it is one of [`WRITE_WORDS`]; everything else is a write.
// ponytail: a function the select calls can still write (an Oracle
// autonomous transaction, a Postgres function); the real guard is a
// read-only login, or a read-only connection on Oracle and Postgres.
pub fn writes(sql: &str, kind: Kind) -> bool {
    let mut words = tokens(sql, kind)
        .into_iter()
        .filter_map(|token| match token {
            Token::Word(range) => Some(sql[range].to_ascii_lowercase()),
            _ => None,
        });
    match words.next().as_deref() {
        Some("select" | "with") => {}
        Some("show" | "table" | "values" | "explain") if kind == Kind::Postgres => {}
        _ => return true,
    }
    let words: Vec<String> = words.collect();
    words.iter().enumerate().any(|(at, word)| {
        let previous = if at > 0 { words[at - 1].as_str() } else { "" };
        let next = words.get(at + 1).map_or("", String::as_str);
        // `OPENROWSET(BULK 'file', …)` reads a file (Synapse reads the lake
        // so); OPENROWSET with a provider can run a query elsewhere.
        let reads_a_file = (word == "openrowset" && next == "bulk")
            || (word == "bulk" && previous == "openrowset");
        // SQL Server's `next value for seq` moves the sequence; Postgres's
        // `for share` and `for key share` lock rows, which a read-only
        // transaction refuses.
        (WRITE_WORDS.contains(&word.as_str()) && !reads_a_file)
            || (previous == "next" && word == "value")
            || (kind == Kind::Postgres
                && (POSTGRES_WRITE_WORDS.contains(&word.as_str())
                    || (word == "share" && matches!(previous, "for" | "key"))))
    })
}

/// Whether `sql` runs a transaction of its own (BEGIN TRAN, COMMIT,
/// ROLLBACK, SAVE TRAN, SET IMPLICIT_TRANSACTIONS or XACT_ABORT; Postgres's
/// BEGIN, START TRANSACTION, END and ABORT): then what ran before a failure
/// stays only if no open transaction held it.
pub fn controls_transaction(sql: &str, kind: Kind) -> bool {
    let words: Vec<String> = tokens(sql, kind)
        .into_iter()
        .filter_map(|token| match token {
            Token::Word(range) => Some(sql[range].to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if kind == Kind::Postgres
        && matches!(
            words.first().map(String::as_str),
            Some("begin" | "start" | "end" | "abort")
        )
    {
        return true;
    }
    words.iter().enumerate().any(|(at, word)| {
        let next = words.get(at + 1).map_or("", String::as_str);
        match word.as_str() {
            "commit" | "rollback" => true,
            "begin" | "save" => next.starts_with("tran") || next == "distributed",
            "set" => matches!(next, "implicit_transactions" | "xact_abort"),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_synapse_select_over_lake_files_only_reads() {
        let sql = "SELECT r.filepath(1) AS date, count(*) AS orders FROM OPENROWSET(BULK \
                   'https://contosolake.dfs.core.windows.net/raw/sales/orders/date=*/*.parquet', \
                   FORMAT = 'PARQUET') AS r GROUP BY r.filepath(1)";
        assert!(!writes(sql, Kind::Mssql));
        for sql in [
            "select * from openrowset('SQLNCLI', 'server=x', 'exec dbo.purge')",
            "select * from openquery(linked, 'select 1')",
            "bulk insert t from 'f.csv'",
        ] {
            assert!(writes(sql, Kind::Mssql), "{sql}");
        }
    }

    #[test]
    fn a_statement_that_runs_its_own_transaction_is_told_apart() {
        for sql in [
            "begin tran",
            "BEGIN TRANSACTION t1",
            "update t set a = 1; commit",
            "if @@error <> 0 rollback",
            "save tran before_update",
            "set implicit_transactions on",
            "SET XACT_ABORT ON",
            "begin distributed transaction",
        ] {
            assert!(controls_transaction(sql, Kind::Mssql), "{sql}");
        }
        for sql in [
            "begin select 1 end",
            "update t set commit_id = 1",
            "select 'commit' as word",
            "set nocount on",
            "-- rollback\nselect 1",
        ] {
            assert!(!controls_transaction(sql, Kind::Mssql), "{sql}");
        }
    }

    fn mssql(text: &str) -> Vec<String> {
        split(text, Kind::Mssql)
    }

    fn oracle(text: &str) -> Vec<String> {
        split(text, Kind::Oracle)
    }

    fn postgres(text: &str) -> Vec<String> {
        split(text, Kind::Postgres)
    }

    #[test]
    fn postgres_cuts_at_semicolons_outside_strings_dollar_quotes_and_comments() {
        assert_eq!(
            postgres("select 1; select ';' as a;\n-- two; three\nselect \"a;b\" from t;"),
            ["select 1", "select ';' as a", "select \"a;b\" from t"]
        );
        assert_eq!(
            postgres("select E'it\\'s; fine', 'it''s; fine'; select 2"),
            ["select E'it\\'s; fine', 'it''s; fine'", "select 2"]
        );
        assert_eq!(
            postgres("/* outer /* inner; */ still; */ select 1; select 2"),
            ["select 1", "select 2"],
            "Postgres comments nest"
        );
        assert_eq!(
            postgres("select 1\n/\nselect 2;\ngo\n"),
            ["select 1\n/\nselect 2", "go"],
            "neither a slash nor GO is a separator"
        );
        assert_eq!(postgres(";;\n-- note\n"), Vec::<String>::new());
    }

    #[test]
    fn a_postgres_function_body_is_one_statement_however_it_is_quoted() {
        let plpgsql = "create or replace function bench.f(n int) returns int language plpgsql as $$\nbegin\n  if n > 0 then return 1; end if;\n  return 0;\nend;\n$$";
        let tagged =
            "create function g() returns text as $body$ select 'a;$$;b' $body$ language sql";
        let block = "do $do$ begin perform 1; end $do$";
        assert_eq!(
            postgres(&format!(
                "{plpgsql};\n{tagged};\n{block};\nselect $1, a$b from t;"
            )),
            [plpgsql, tagged, block, "select $1, a$b from t"]
        );
        let atomic = "create procedure bench.p(n int) language sql begin atomic\n  insert into t values (n);\n  select case when n > 0 then 1 else 0 end;\nend";
        assert_eq!(
            postgres(&format!("{atomic};\nselect 1;")),
            [atomic, "select 1"]
        );
        assert_eq!(
            postgres("select 'unclosed $$; select 2"),
            ["select 'unclosed $$; select 2"]
        );
        assert_eq!(
            postgres("select $$unclosed; select 2"),
            ["select $$unclosed; select 2"]
        );
    }

    #[test]
    fn postgres_reads_are_selects_shows_and_explains_and_its_locks_and_setters_are_writes() {
        for read in [
            "select * from t where note = 'delete me'",
            "show search_path",
            "explain select * from t",
            "explain (analyze, buffers) select count(*) from t",
            "table t",
            "values (1), (2)",
            "select $$drop table t$$",
        ] {
            assert!(!writes(read, Kind::Postgres), "{read}");
        }
        for write in [
            "select * from t for update skip locked",
            "explain analyze delete from t",
            "select * from t for share",
            "select * from t for key share",
            "select setval('s', 1)",
            "select set_config('default_transaction_read_only', 'off', false)",
            "select pg_terminate_backend(42)",
            "select nextval('s')",
            "with gone as (delete from t returning *) select * from gone",
            "do $$ begin perform 1; end $$",
            "copy t to stdout",
            "vacuum t",
            "begin",
        ] {
            assert!(writes(write, Kind::Postgres), "{write}");
        }
        assert!(
            writes("show x", Kind::Mssql),
            "show is a read on Postgres only"
        );
    }

    #[test]
    fn a_postgres_script_running_its_own_transaction_is_told_apart() {
        for sql in [
            "begin",
            "BEGIN ISOLATION LEVEL SERIALIZABLE",
            "start transaction",
            "commit",
            "end",
            "abort",
            "rollback to savepoint a",
        ] {
            assert!(controls_transaction(sql, Kind::Postgres), "{sql}");
        }
        for sql in [
            "create procedure p() language sql begin atomic select 1; end",
            "select 'begin'",
            "do $$ begin commit; end $$",
        ] {
            assert!(!controls_transaction(sql, Kind::Postgres), "{sql}");
        }
    }

    #[test]
    fn sql_server_cuts_only_at_go_lines_so_a_declare_keeps_its_select() {
        assert_eq!(
            mssql("declare @x int = 1\n\nselect @x"),
            ["declare @x int = 1\n\nselect @x"],
            "a blank line is not the end of a batch"
        );
        assert_eq!(
            mssql("select 1; select 2\ngo\nselect 3\n  GO  \n\nselect 4"),
            ["select 1; select 2", "select 3", "select 4"]
        );
        assert_eq!(
            mssql("select 1\n/\nselect 2"),
            ["select 1\n/\nselect 2"],
            "a slash is Oracle's"
        );
        assert_eq!(
            mssql("select 'a\ngo\nb'\ngo\n/* x\ngo\n*/ select 2"),
            ["select 'a\ngo\nb'", "select 2"],
            "a GO inside a string or a comment is text, and a statement starts at its code"
        );
        assert_eq!(
            mssql("/* outer /* inner */\ngo\n*/ select 1"),
            ["select 1"],
            "T-SQL comments nest"
        );
        assert_eq!(mssql("go\n\n  \ngo\n;"), Vec::<String>::new());
        assert_eq!(mssql("-- only a note"), Vec::<String>::new());
    }

    #[test]
    fn oracle_cuts_at_semicolons_and_slash_lines_outside_quotes_and_comments() {
        assert_eq!(
            oracle("select 1 a from dual; select ';' b from dual; -- two"),
            ["select 1 a from dual", "select ';' b from dual"]
        );
        assert_eq!(
            oracle("select 1\nfrom dual\n/\nselect 2 from dual"),
            ["select 1\nfrom dual", "select 2 from dual"]
        );
        assert_eq!(
            oracle("select q'[it's; fine]' a from dual; select nq'{x;}' from dual;"),
            [
                "select q'[it's; fine]' a from dual",
                "select nq'{x;}' from dual"
            ]
        );
        assert_eq!(
            oracle("select 1 /* ; */ from dual -- ;\n; select \"a;b\" from t"),
            ["select 1 /* ; */ from dual -- ;", "select \"a;b\" from t"]
        );
        assert_eq!(
            oracle("delete from t\n\nwhere a = 999;"),
            ["delete from t\n\nwhere a = 999"],
            "never a delete without its where"
        );
        assert_eq!(oracle(";;\n-- note\n/\n"), Vec::<String>::new());
    }

    #[test]
    fn a_plsql_block_is_one_statement_however_many_semicolons_it_has() {
        let block = "declare\n  v number;\nbegin\n  for row in (select 1 from dual) loop\n    begin\n      select count(*) into v from t;\n    end;\n  end loop;\n  if v is null then v := 0; end if;\n  case v when 0 then null; else null; end case;\nend;";
        assert_eq!(
            oracle(&format!("{block}\n/\nselect v from dual")),
            [block, "select v from dual"]
        );
        assert_eq!(
            oracle(&format!("{block}\nselect v from dual;")),
            [block, "select v from dual"],
            "its own end closes it without a slash"
        );
        assert_eq!(
            oracle("select 1 from dual; begin null; end; select 2 from dual"),
            [
                "select 1 from dual",
                "begin null; end;",
                "select 2 from dual"
            ]
        );
        assert_eq!(
            oracle("/* why */ begin\n  x := case when a then 1 end;\nend;\nselect 2 from dual;"),
            [
                "begin\n  x := case when a then 1 end;\nend;",
                "select 2 from dual"
            ],
            "a case expression's end is not the block's"
        );
        assert_eq!(
            oracle(
                "begin\n  begin null; end;\n  if x then null; end if;\nend;\nselect 1 from dual"
            ),
            [
                "begin\n  begin null; end;\n  if x then null; end if;\nend;",
                "select 1 from dual"
            ],
            "an end followed by if on its next line is not an end if"
        );
        assert_eq!(
            oracle("select beginning, ending from t;"),
            ["select beginning, ending from t"]
        );
    }

    #[test]
    fn a_stored_program_runs_through_its_declarations_to_its_own_end() {
        let procedure = "create or replace procedure p is\n  v number;\n  cursor c is select 1 from dual;\n  function f return number is\n  begin\n    return 1;\n  end;\n  procedure later;\nbegin\n  null;\nend p;";
        assert_eq!(
            oracle(&format!("{procedure}\nselect 1 from dual;")),
            [procedure, "select 1 from dual"]
        );
        let spec = "create package k as\n  type t is table of number;\n  procedure a;\n  function b return number;\nend k;";
        let body = "create or replace editionable package body k as\n  procedure a is\n  begin\n    null;\n  end;\n  function b return number is begin return 1; end;\nbegin\n  a;\nend k;";
        assert_eq!(
            oracle(&format!("{spec}\n{body}\nselect 1 from dual;")),
            [spec, body, "select 1 from dual"]
        );
        let declared = "declare\n  procedure p is\n  begin\n    null;\n  end;\nbegin\n  p;\nend;";
        assert_eq!(
            oracle(&format!("{declared}\nselect 1 from dual;")),
            [declared, "select 1 from dual"]
        );
        let trigger =
            "create trigger t before insert on x for each row\nbegin\n  :new.id := 1;\nend;";
        let type_body = "create type body tb as\n  member function f return number is begin return 1; end;\nend;";
        assert_eq!(
            oracle(&format!(
                "{trigger}\n{type_body}\ncreate table z (id number);"
            )),
            [trigger, type_body, "create table z (id number)"]
        );
    }

    #[test]
    fn only_a_leading_select_or_with_is_a_read() {
        for read in [
            "select 1",
            "  -- note\n/* more */ SELECT * from t where name = 'delete me'",
            "with x as (select 1 a from dual) select * from x",
            "(select 1) union (select 2)",
            "select [update], \"delete\", update_count, @set from t",
            "select * from sys.dm_exec_requests",
            "select * from t order by 1 offset 0 rows fetch next 10 rows only",
        ] {
            assert!(!writes(read, Kind::Mssql), "{read}");
        }
        for write in [
            "insert into t values (1)",
            "update t set a = 1",
            "with x as (select id from t) delete from t where id in (select id from x)",
            "with x as (select 1 a) merge into t using x on 1 = 0 when not matched then insert values (1);",
            "select * into #copy from t",
            "select 1; drop table t",
            "select 1 exec sp_who",
            "declare @x int = 1\n\nselect @x",
            "exec sp_help",
            "sp_help",
            "begin tran",
            "-- read\ncreate table t (a int)",
            "select * from t with (updlock)",
            "select next value for dbo.seq",
            "",
        ] {
            assert!(writes(write, Kind::Mssql), "{write}");
        }
        assert!(writes("select * from t for update", Kind::Oracle));
        assert!(writes("SELECT id FROM t FOR UPDATE NOWAIT", Kind::Oracle));
        assert!(writes("select s.nextval from dual", Kind::Oracle));
        assert!(writes("begin null; end;", Kind::Oracle));
        assert!(!writes("select q'[drop table t]' from dual", Kind::Oracle));
    }

    #[test]
    fn statements_carry_their_classification() {
        assert_eq!(
            statements("select 1 from dual; delete from t", Kind::Oracle),
            [
                Statement {
                    sql: "select 1 from dual".to_owned(),
                    writes: false
                },
                Statement {
                    sql: "delete from t".to_owned(),
                    writes: true
                },
            ]
        );
    }
}

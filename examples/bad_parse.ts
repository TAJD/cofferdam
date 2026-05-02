// Deliberately broken TS — used to verify Warning.ParseError handling.
function ok() { return 1 }

class Broken {
  // unterminated string + missing brace
  badField = "open quote
}

const x = { y:

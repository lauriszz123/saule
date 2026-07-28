# Brainfuck

A Saule project. Run with:

```sh
saule run
```

It takes the program to interpret as an argument. Script arguments go after
`--`, so the CLI forwards the filename through to `Os.args()` instead of
trying to run it as Saule:

```sh
saule run -- test.bf
saule run -- 400quine.bf
saule run -- -d          # the hello world embedded in the interpreter
```

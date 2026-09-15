say = function(...) {
    cat(..., "\n", sep = "")
    flush(stdout())
}

say("about to call box::use()")

box::use(
    # Custom `{purrr}` package
    fp = fpurrr,
    fpurrr/map,

    # Custom "stats" module
    stm = statmodule,
    statmodule/random/draw
)

say("box::use() returned")

say("")
say("== Using the attached entire package ==")

say("")
say("-- `{fpurrr}` under the alias `fp` --")

say("fp$map$call(1:5, sqrt)")
print(fp$map$call(1:5, sqrt))

say("")
say("fp$map$call@dbl(1:5, sqrt) (typed dispatch)")
print(fp$map$call@dbl(1:5, sqrt))

say("")
say("-- `{statmodule}` under the alias `stm` --")

say("stm$regression$ols$linear_reg(mtcars, mpg ~ wt + disp)")
print(stm$regression$ols$linear_reg(mtcars, mpg ~ wt + disp))

say("")
say("== Using submodules imported directly ==")

say("")
say("map$call(1:5, sqrt)")
print(map$call(1:5, sqrt))

say("")
say("draw$normal(10, 5, 1)")
print(draw$normal(10, 5, 1))

say("")
say("== Assertions ==")

stopifnot(identical(fp$map$call@dbl(1:5, sqrt), sqrt(1:5)))
stopifnot(identical(map$call(1:5, sqrt), fp$map$call(1:5, sqrt)))
stopifnot(length(draw$normal(10, 5, 1)) == 10)

say("")
say("box successfully imported the carrier-installed modules.")
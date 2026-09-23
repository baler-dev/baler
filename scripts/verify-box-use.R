say = function(...) {
    cat(..., "\n", sep = "")
    flush(stdout())
}

say("about to call box::use()")

box::use(
    # Custom `{convert}` for conversion
    cv = convert,
    convert/mass,

    # "Calculus in R" module
    rcalc,
    rcalc/quad
)

say("box::use() returned")

say("")
say("== Using the attached entire package ==")

say("")
say("-- `{convert}` under the alias `cv` --")

say("cv$mass$convert_mass(1:5, \"oz\", \"g\")")
print(cv$mass$convert_mass(1:5, "oz", "g"))

say("")
say("-- `{rcalc}` --")

say("rcalc$quad(function(x) 3 * x^2, 0, 3)")
print(rcalc$quad(function(x) 3 * x^2, 0, 3))

say("")
say("== Using submodules imported directly ==")

say("")
say("mass$convert_mass(1:5, \"oz\", \"g\")")
print(mass$convert_mass(1:5, "oz", "g"))

say("")
say("quad$quad(function(x) 3 * x^2, 0, 3)")
print(quad$quad(function(x) 3 * x^2, 0, 3))

say("")
say("box successfully imported the carrier-installed modules.")

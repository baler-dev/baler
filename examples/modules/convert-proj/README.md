
# Convert package

This is the first `box-bale` module ever made, only used as a demo,
where it contains the small collection of example codes featuring
conversion units.

## Installation

See the [{baler} installation
guide](https://baler-dev.github.io/baler/installation.html) for the
installation details first. Then, for the meantime, install the patched
forked `{box}` version:

``` r
# install.packages('pak')
pak::pak("joshuamarie/box@feature/baler-module-support")
```

Then, install this package via following:

``` bash
baler install --git https://github.com/baler-dev/baler --branch examples/modules/convert-proj
```

## Usage

This module IS technically an R package, just not being composed on a
traditional CRAN-style package system, and it has to be attached through
`box::use()` and has similar paradigm as Python’s.

It has several ways to import the `convert` module:

``` r
box::use(
    convert,
    cv = convert, 
    convert/mass,
    tmp = convert/temp,
    # ...  and there are many more
)
```

### Example

Let’s run an example function from the imported `cv`:

``` r
cv$mass$mass_conversion_table(1000)
#> 
#>   Mass conversion table for 1000 kg
#>   -----------------------------------
#>   mg         : 1e+09
#>   g          : 1e+06
#>   kg         : 1000
#>   t          : 1
#>   oz         : 35274
#>   lb         : 2204.62
#>   st         : 157.473
#>   ton_us     : 1.10231
#>   ct         : 5e+06
#>   amu        : 6.02214e+29
#>   -----------------------------------
```

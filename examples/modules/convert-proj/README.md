# Convert package

This is an example `{box}` module package managed by `{baler}`, where it contains the collection of example codes featuring conversion units. 

## Installation

See the [{baler} installation guide](https://joshuamarie.com/baler/installation.html) for the installation details first. Then, for the meantime, install the patched forked `{box}` version:

``` r
# install.packages('pak')
pak::pak("joshuamarie/box@feature/baler-module-support")
```

Then, install this package via following:

``` bash
baler install gh:joshuamarie/baler/tree/main/examples/modules/convert-proj
```

## Usage

This module IS an R package, not on a traditional CRAN-style package system, and it has to be attached through `box::use()` and has similar paradigm as Python's. 

It has several ways to import the `convert` module:

``` r
box::use(
    convert,
    cv = convert, 
    convert/mass,
    tmp = convert/temp,
    ...
)
```

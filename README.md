# digitalis

digitalis is a music server + some various frontends for the server

extremely wip, nothing works

## development environment setup

1. install [Nix](https://nixos.org/) on your system (you don't need to install NixOS)
2. enable [flakes and the nix command](https://wiki.nixos.org/wiki/Flakes)
3. enter the devShell

```sh
nix develop
```

## build


### server
```sh
cargo r --bin digitalis-server
```
### tui client
```sh
cargo r --bin digitalis-client
```
### react native client
```sh
cd react-native
npm i
npx expo start
```

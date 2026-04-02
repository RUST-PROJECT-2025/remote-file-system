# remote-file-system

# To start container

```shell
cd nome_cartella_progetto

# per buildare l'immagine
docker build -t rust-fuse-dev .

# dare i permessi ed eseguire lo script 
chmod +x run.sh
./run.sh
````

``` shell
# Entra nel container
docker exec -it rfs-client bash

# Spostati nel mount point
cd /mnt/remote-fs
```



# Guida all'avvio rapido
Segui questi passaggi per avviare l'intera infrastruttura (Client, Server e Rete virtuale) in un ambiente Linux isolato tramite Docker.

1. Build dell'immagine
Dalla cartella principale del progetto, compila l'ambiente di sviluppo:

```Bash
docker build -t rust-fuse-dev .
```

2. Esecuzione dello script di orchestrazione
Lo script run.sh configura automaticamente la rete Docker, avvia il server in background e il client FUSE in modalità interattiva:

```Bash
chmod +x run.sh
./run.sh
```

3. Utilizzo del File System
Mentre il client è attivo nel primo terminale, apri una nuova finestra del terminale sul tuo computer per interagire con il file system:

```Bash
# Entra nel container client
docker exec -it rfs-client bash

# Spostati nel mount point
cd /mnt/remote-fs

# Ora puoi usare i normali comandi Linux (ls, touch, echo, cat, cp, ecc.)
echo "Hello Remote World" > test.txt
cat test.txt
```

# 🛠 Comandi Utili
## Monitoraggio
Log del Server: docker logs -f rfs-server

Log del Client: Sono visibili direttamente nel terminale dove hai lanciato ./run.sh.



# Vecchi comandi 

```shell
# per runnare il container su mac
docker run -it \
    --privileged \
    --cap-add=SYS_PTRACE \
    --device /dev/fuse \
    -v $(pwd):/workspace \
    --name remote_fs_container_2 \
    rust-fuse-dev

# per runnare il container su windows
docker run -it `
    --privileged `
    --cap-add=SYS_PTRACE `
    -v ${PWD}:/workspace `
    --name remote_fs_container_2 `
    rust-fuse-dev

```



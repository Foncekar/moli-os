# Netatmo (`moli-netatmo`)

Pilote natif pour les caméras Netatmo Presence (`NOC`), Welcome (`NACamera`) et sonnette (`NDB`) : vignettes en direct sur le réseau local (`snapshot_720.jpg`), détections (personne, véhicule, animal, mouvement), surveillance et projecteur (`auto`, `on`, `off`).

Il pilote aussi les volets roulants Bubendorff (iDiamant, module `NBR`) par la même API : `position` (sens `position`, 0 à 100 %, 100 = ouvert), `control` (`open`, `stop`, `close`) et `moving` (lecture, `up`, `down`, `stopped`). Portées du jeton en plus, à demander seulement pour les volets : `read_bubendorff`, `write_bubendorff`.

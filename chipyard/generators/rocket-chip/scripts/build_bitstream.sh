# /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_build.yaml
# /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim/deploy/config_runtime.yaml
# 这两个只需要改名字
# /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim-staging/sample_config_hwdb.yaml
# /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim-staging/sample_config_build_recipes.yaml
# 这两个需要添加名字




cd /home/qxh/CHIPYARD_TEST/chipyard/sims/firesim
source sourceme-manager.sh
cd /home/qxh/CHIPYARD_TEST/chipyard/generators/rocket-chip
firesim buildbitstream -r ${CY_DIR}/sims/firesim-staging/sample_config_build_recipes.yaml
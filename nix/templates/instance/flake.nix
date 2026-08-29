{
  description = "A declarative CHAP instance";

  inputs.chap.url = "github:luizribeiro/chap";

  outputs =
    { chap, ... }:
    chap.inputs.flake-utils.lib.eachDefaultSystem (
      system:
      {
        packages.default = chap.lib.${system}.mkChap {
          name = "my-chap";
          plugins = {
            # openai = {
            #   plugin = chap.packages.${system}.plugin-openai-compatible;
            #   settings = {
            #     base_url = "http://127.0.0.1:8080/v1";
            #     model = "local-model";
            #   };
            # };

            # greeter = {
            #   plugin = chap.lib.${system}.buildChapPlugin {
            #     pname = "my-plugin";
            #     src = ./my-plugin;
            #   };
            #   settings.greeting = "Hi";
            # };
          };
        };
      }
    );
}

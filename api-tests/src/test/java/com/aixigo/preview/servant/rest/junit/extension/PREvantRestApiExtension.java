package com.aixigo.preview.servant.rest.junit.extension;

/*-
 * ========================LICENSE_START=================================
 * PREvant REST API Integration Tests
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 * 
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 * 
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */


import org.junit.jupiter.api.extension.*;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.testcontainers.containers.BindMode;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.containers.output.OutputFrame;

import java.net.URI;
import java.util.Objects;
import java.util.function.Consumer;

import static org.apache.commons.lang3.StringUtils.isBlank;

public class PREvantRestApiExtension implements BeforeAllCallback, AfterAllCallback, ParameterResolver {

    private final GenericContainer prevantRestApiContainer = new GenericContainer("aixigo/prevant")
            .withFileSystemBind("/var/run/docker.sock", "/var/run/docker.sock", BindMode.READ_WRITE)
            .withLabel("traefik.frontend.rule", "ReplacePathRegex: ^/api(.*) /$1;PathPrefix:/api;")
            .withLogConsumer(new Consumer<OutputFrame>() {

                private final Logger LOGGER = LoggerFactory.getLogger("rest api");

                @Override
                public void accept(OutputFrame outputFrame) {
                    log(LOGGER, outputFrame);
                }
            });

    private final GenericContainer traefikContainer = new GenericContainer("traefik")
            .withFileSystemBind("/var/run/docker.sock", "/var/run/docker.sock", BindMode.READ_WRITE)
            .withCommand("--api --docker")
            .withLogConsumer(new Consumer<OutputFrame>() {

                private final Logger LOGGER = LoggerFactory.getLogger("traefik");

                @Override
                public void accept(OutputFrame outputFrame) {
                    log(LOGGER, outputFrame);
                }
            });

    private static void log(Logger logger, OutputFrame outputFrame) {
        String utf8String = outputFrame.getUtf8String();
        if (!isBlank(utf8String)) {
            logger.info(outputFrame.getUtf8String().trim());
        }
    }

    public void afterAll(ExtensionContext extensionContext) {
        prevantRestApiContainer.stop();
        traefikContainer.stop();
    }

    public void beforeAll(ExtensionContext extensionContext) {
        prevantRestApiContainer.start();
        traefikContainer.start();
    }

    public boolean supportsParameter(ParameterContext parameterContext, ExtensionContext extensionContext) throws ParameterResolutionException {
        return Objects.equals(parameterContext.getParameter().getType(), URI.class);
    }

    public Object resolveParameter(ParameterContext parameterContext, ExtensionContext extensionContext) throws ParameterResolutionException {
        return URI.create("http://localhost:" + traefikContainer.getMappedPort(80));
    }
}

package dev.lapis.mobile.ui

import kotlin.test.Test
import kotlin.test.assertEquals

class MobileFoundationTest {
    @Test
    fun application_name_is_stable() {
        assertEquals("Lapis", ApplicationName)
    }
}
